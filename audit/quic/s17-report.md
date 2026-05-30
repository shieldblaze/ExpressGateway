# SESSION 17 — report (CF-S16-RELAY-STALL → finish Mode B → promote)

Branch `feature/quic-proxy-s17` off `feature/quic-proxy-s16 @ b1e9e621`.
Box: 8 cores (stall is scheduling-sensitive — diagnosed/verified at production core count).

Standing rules in force: R1 (×3 green), R2 (mechanism-classify every failure),
R5 (author≠verifier), R13 (layered timing verification), **R15 (no result from an
incomplete job — every claim cites a completed, read-to-end log)**.

---

## Phase 0 — baseline + hygiene  ✅ COMPLETE

### Hygiene
- Base tip confirmed: `feature/quic-proxy-s17 @ b1e9e621` (S16 tip, NOT promoted — correct).
- `ps aux`: no cargo/rustc strays, no S16 heartbeat cron, no disk-guard, no leftover
  worktrees, no root-owned artifacts. Only the system `cron -f` daemon + this session.
- Disk: stale S16 build cache cleaned (`eg-target/debug`, 28 GB) → **51 GB free** (≥25 GB ✔).
- Quarantined a prior incomplete-pass's `s17-logs/*` artifacts (07:06–07:18) — NOT results
  (R15) — then removed them.

### Gates (each foreground/background-to-completion, read to end — R15; logs in `s17-logs/`)
| gate | command | result | log |
|---|---|---|---|
| compile | `cargo test --workspace --all-features --no-run` | **exit 0**, no warnings | `p0_compile.log` |
| fmt | `cargo fmt --check` | **exit 0** clean | `p0_fmt.log` |
| clippy | `cargo clippy --all-targets --all-features -- -D warnings` | **exit 0** clean | `p0_clippy.log` |

### R1 ×3 baseline — `cargo test --workspace --all-features` (sequential ×3)
Logs `p0_test_run{1,2,3}.log`. Per-binary Running→FAILED pairing (authoritative):

| run | failed binary | failing test | mechanism | class |
|---|---|---|---|---|
| 1 | `s16_b2_backpressure` | `…client_throttled_then_complete_on_resume` | `Elapsed(())` after **90 s** waiting for `done_rx` (resumed transfer never completes) — `s16_b2_backpressure.rs:788` | **LIVENESS STALL** (= CF-S16-RELAY-STALL class) |
| 2 | `s16_b3_reset_propagation_verify` | `forward_client_reset_propagates_with_code_conn_stays_up` | `unwrap()` on `Err(Done)` after **0.34 s** — the **client's own** `stream_send(SIBLING_STREAM,…)` at `:627` | **TEST-HARNESS FLAKE** (fast; distinct) |
| 3 | — (all pass) | — | — | — |

**Crucial reframe (R2 mechanism-classified):**
- **Everything OUTSIDE the Mode B `s16_*` suite is green across all 3 runs** — S1–S15,
  the 9 matrix cells, H3 termination, Mode A passthrough, XDP datapath, foundation,
  security. **No regression (R3) anywhere outside Mode B.** (sanity grep: "all FAILED
  binaries are `s16_*`".)
- **CF-S16-RELAY-STALL is NOT confined to `s16_b2_multistream`.** It is a *general* Mode B
  relay-liveness defect: this round it stalled the **backpressure-resume** path
  (`s16_b2_backpressure`, 90 s Elapsed) while `s16_b2_multistream` passed ×3 (its ~22% did
  not fire in 3 samples — expected). The author's own comment at `:784` ("sub-second
  isolated") confirms 90 s is a NEVER-completes, not a slow-completes. **This makes the
  stall squarely a production liveness bug** (backpressure-resume is core relay behaviour),
  not a property of one test.

### Findings opened at baseline
- **CF-S16-RELAY-STALL** (BLOCKER, OPEN) — general Mode B relay liveness stall. → Phase 1.
- **CF-S17-B3-VERIFY-DONE-UNWRAP** (test fragility, OPEN) — `s16_b3_reset_propagation_verify.rs:627`
  does `client.stream_send(SIBLING_STREAM, SIBLING_PAYLOAD, true).unwrap()`; quiche may
  return `Err(Done)` transiently (stream-grant / connection-flow-control not yet available
  right after the FWD_STREAM reset). FAST fail, client-local (no relay round-trip involved)
  ⇒ NOT the stall, NOT a production defect. Tractable test-only fix (pump+retry the sibling
  open, as the test already does for recv). Will be fixed in Phase 2 with author≠verifier.
  Per R4 it is tracked into the gate, never asterisked.

_Phase 0 verdict: baseline established; non-Mode-B workspace clean ×3; Mode B suite
non-deterministic via one liveness stall (general) + one fast test flake. Proceed to
Phase 1 diagnosis (HARD CAP: one focused pass)._
