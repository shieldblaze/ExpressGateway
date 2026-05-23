# SESSION 7 — H-Matrix Report

**Branch:** `feature/h-matrix-s7` → tip `0cd584e7` (H3→H3 BUILT integrated).
**Base:** `main @ 2c62896f` (S6 promoted) — confirmed.
**Verdict:** **SESSION 7 COMPLETE** — H3→H3 BUILT & verified (3 of 9 cells); H2→H1 honest-stopped with R8 DRAFT plan authored; Phase 3 gate PASS (R1×3 1155/0/16 deterministic, R10 gauges ran, clippy/fmt clean, R3 intact). Promoted to `main` per R11.

> **This was a RESUMED session.** Substantial S7 work pre-existed on disk
> (worktrees `s7-builder1/2`, `s7-verifier`). The report reconciles the
> prompt's "build H3→H3 from the plan" framing with on-disk reality and
> continues from the verified state rather than redoing it.

---

## 1. Phase 0 — baseline & state reconciliation

- **Base tip**: `main @ 2c62896f` confirmed (matches brief).
  `feature/h-matrix-s7 @ e641f2b4` = main + the P0 env-fix only.
- **Resumed state discovered** (not a fresh build):
  - `s7/builder-1 @ 8d0fe450`: full H3→H3 stack (J1–J5 + fixes F-S7-1/2/4/6),
    every increment **independently verified PASS** by `s7/verifier`
    (real-wire genuine quiche path; non-vacuous memory bound 73728 ≪ 262656,
    body-independent; backpressure both directions; smuggling case-7).
  - Prior verifier verdict (`s7/verifier @ 105a3e57`): **NOT BUILT — sole
    blocker = §7 coverage** (session code 69.08%, `h3_to_h3_stream_resp`
    67.91%, both < 80%), characterized as a *test gap*, not a src defect.
  - **648 lines of UNCOMMITTED tests** sat in the builder-1 worktree — 8
    adversarial real-wire cases a prior builder added to close the gap but
    never committed/built/measured.
- **Phase-0 verification of the WIP** (lead, this session):
  - Tree compiles with the uncommitted tests (cold build 68 s, shared
    `eg-target`).
  - `h3_h3_stream_e2e` suite **15/15 pass** (40.66 s).
  - Lead review of the 8 cases: genuinely real-wire & non-vacuous (upstream
    raw-writes unknown frame `0x21`; QPACK-encodes real trailing-HEADERS incl.
    `:illegal` pseudo; client counts HEADERS frames + collects trailer names).
- Toolchain 1.85.1; cargo-llvm-cov 0.8.7 + llvm-tools installed → **coverage
  runs locally, no offline fallback**. 8 cores, user `ubuntu`, ens5.

> **Phase-0 green basis (R1):** the integrated point `e641f2b4` was
> independently re-baselined GREEN R1×3 (1132/0/16 deterministic) by the prior
> verifier (`s7/verifier` 1d0ac829). The binding fresh green for the new
> integration point (`0cd584e7`) is **Phase 3 §5**, re-run this session — not
> trusted from prior.

### Environment / standing-rule conflict (disclosed, not asterisked)
- **R9 ">50 GB free" is physically unsatisfiable** on this 67 GB volume. Held a
  safe margin instead: reclaimed the stale 22 GB feature-target and used a
  single shared `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target` across all
  worktrees (disk forces serialized builds — honored). Free space stayed
  42–50 GB throughout. This is an environment vs. rule conflict, reported
  honestly rather than papered over.

---

## 2. Phase 1 — H3→H3 → **BUILT** ✅ (3 of 9 cells)

Per the lead-approved `s6-h3h3-plan.md` (J1–J5; net-new M-C bounded
QUIC-stream flow-control upstream connector; H3 ingress reuses verified M-A).
Plan re-confirmed VALID vs. the current tree (builder-1 `28710c18` — 2 doc
drifts only, no design/R8 change).

### 2.1 Increments (each plan-approved → build → independent verify → commit)
| Inc | Commit | What | Verifier |
|---|---|---|---|
| J1 | 0ce98bad | M-C recv half + orchestrator skeleton | PASS (b75d700e) |
| J2 | 8fef9e9f | M-C request/send half — streaming request-DATA pump | PASS (57506d7c) |
| J3 | e07b6f63 | rewire H3→H3 actor branch to streaming + delete dead code | PASS (906b8901) |
| F-S7-1 | d17e51c4 | non-panicking `.get()` (crate-root indexing_slicing) | PASS (04bcc274) |
| J4 | e42a9b4e | real-wire H3→H3 suite (exposed F-S7-2) | — |
| F-S7-2/J5 | fedb5cf4 | discriminate benign stream-collected vs genuine reset | PASS (587a7fcb) |
| F-S7-6 | b24d9bfd | event-driven idle deadline (replaces 5 s wall-clock) | PASS (7fb2287b) |
| F-S7-4 | 8d0fe450 | genuine transport-stall makes case-4 backpressure non-vacuous | PASS (4e63ec2c) |
| **§7 close** | **0cd584e7** | **+12 real-wire cases → session coverage ≥80%** | **PASS (this session)** |

### 2.2 The §7 coverage closure (this session's net-new work)
- Binding metric (METHOD.md §7) = session code = `h3_bridge.rs` fns
  `h3_to_h3_stream_resp` [2773-3540], `j2_req_event_action` [3541-3557],
  `check_block_len` [3558-3608], `parse_frame_header` [3623-3636]. Bar **≥80%**.
- The 8 uncommitted cases moved session code 69.08% → **76.97%** (351/456) —
  *still 14 lines short* (lead measured; whole-file `h3_bridge.rs` crossed 80%
  at 80.62% but the **stricter session-code sub-metric did not** — the WIP did
  NOT close the gate). Reported honestly rather than riding the looser
  module gate.
- **builder-1** (author) added **5 more reliably-reachable real-wire cases**
  (`pool_acquire_failure_returns_502`, `client_stop_sending_response_maps_client_gone`,
  `absent_authority_substitutes_sni`, `empty_data_frame_skipped_then_body`,
  `upstream_premature_eof_mid_data_no_clean_fin`) → session code **80.26%**
  (366/456, +15). No src change (A-Q1 boundary held). Committed `0cd584e7`.
- **verifier** (independent, author≠verifier strict) re-measured the binding
  §7 with the canonical `cargo llvm-cov -p lb-quic --features test-gauges`:
  **366/456 = 80.26%, DETERMINISTIC across 3 reruns** (uncovered set
  byte-identical, zero flicker — the +1-line margin is real and stable).
  Verify-doc `ceda5c4a` (appended to `s7-h3h3-verify.md` §8), pushed.

### 2.3 BUILT-bar proofs (verifier `ceda5c4a`, source byte-identical to prior PASS)
- **§1 Real-wire genuine**: real `quiche::connect` client → real listener →
  router → `conn_actor::poll_h3` J3 h3_backend branch → `h3_to_h3_stream_resp`
  → genuine `quiche::accept` upstream on a real UDP socket. Binary bodies.
- **§2/§3 Non-vacuous memory** both dirs: gauge = real crate statics
  `MAX_RETAINED_{BODY,RESP}_BYTES`; retained 73728 ≪ 262656 for 4 MiB,
  body-size-independent (S6 73859 / S7 73728); inverted probe proved the
  assertion load-bearing.
- **§4 Backpressure** both dirs: req-dir genuine `stream_capacity==0` throttle
  (gate fired 2126×, retained 73728 ≪ 4 MiB); resp-dir stalled-client park.
- **§5 Smuggling case-7**: client RST mid-request → `stream_shutdown(Write,
  H3_REQUEST_CANCELLED=0x010c)`, upstream never sees a truncated-as-complete
  request, client never gets clean 200+FIN.
- **§6 Determinism**: 20/20 ×(4 builds), 0 `#[ignore]`, gauge cases compiled
  out without the flag.
- **Non-vacuity of new cases**: PASS all 5 (+prior 8) — load-bearing
  assertions, genuine real-wire.
- **Residual coverage (90 lines) — justified genuinely-unreachable-via-harness**
  (verifier spot-checked 3, SOUND): mid-flight transport-fault arms (quiche
  buffers sends locally — need white-box injection, A-Q1 out-of-scope);
  malformed-varint impossible (`lb_h3::decode_varint` 2-bit prefix has no
  invalid case); encode-fail only on ~2^62 B input; 64 MiB cap impractical.

### 2.4 Honest carry-forward on H3→H3 (CF-COV-S7)
The session-code pass is **thin**: `h3_to_h3_stream_resp` alone is 79.77%; the
total clears 80% only via the small helper fns, headroom **1 line**. Stable
today (deterministic ×3), binding pass now — but any future src edit adding a
reachable uncovered line could drop it below bar. Tracked for S8.

---

## 3. Honest-stop decision — H2→H1 NOT built (plan authored)

**Invoked.** Per the brief's honest-stop clause (lead's delegated budget call,
R7), after H3→H3 verified BUILT the lead judged Phase-2 budget unsafe to build
a second cell to the full bar AND land an honest Phase-3 gate. **"One
fully-verified cell + a clean gate beats two half-done."**

Rationale: H2→H1 is net-new substantial work (M-D bounded H2 ingress + the
h2spec validate-before-forward reconciliation — see §2 of the plan) starting
from its PARTIAL/buffering state, not the 95%-done state H3→H3 was in. The
mandatory Phase-3 full-workspace gate (the long pole) remained ahead.

**Deliverable instead:** `audit/h-matrix/s8-h2h1-plan.md` — a DRAFT R8 plan
(NOT yet R8-approved) capturing the defect (`Limited::collect()` total-cap
buffering at `h2_proxy.rs:1314`), the central tension (R8 streaming vs. the
F-COR-1 A2-2 validate-before-forward h2spec ordering, ≥5 faces), the M-D design
sketch, the verified-egress reuse, the BUILT verification bar, and 4 open
questions (Q-D1..Q-D4) for S8 builder-detailing + lead R8-approval.

---

## 4. Carry-forwards (tracked, not in S7 scope)
- **F-ESC-1** — multi-kernel verifier CI lane (~0.5 d). Still open.
- **N-1** — jumbo-MTU `xdp.frags` deployment doc. Still open.
- **S4-NUANCE-1**, **CF-DEDUP-1**, **CF-COV-1/2** — carried.
- **CF-COV-S7 (new)** — H3→H3 session-code at 1-line headroom (§2.4).

---

## 5. Phase 3 — mandatory gate → **PASS**

Run on the integrated tip `0cd584e7`, FULL workspace, cross-crate (R10 scope),
`--all-features` (⊇ `test-gauges`). Log: `/home/ubuntu/Code/s7-gate/`.

| Check | Command | Result |
|---|---|---|
| fmt (R1) | `cargo fmt --check` | exit 0 — clean |
| clippy (R1) | `cargo clippy --all-targets --all-features -- -D warnings` | exit 0 — clean |
| R1×3 test | `cargo test --workspace --all-features --no-fail-fast` ×3 | **1155 / 0 / 16** all three runs — **DETERMINISTIC** |
| R10 gauge-ran | gauge-only memory cases present in output | `memory_bounded_through_stalled_{backend,client,upstream}` + `resp_retained_ceiling_is_sound…` **executed** (not silently skipped) |
| ignored set | run1 ignored list | **16** — unchanged vs S6 baseline (no new suppressions) |
| R3 no-regression | prior BUILT + PARTIAL suites | `h3_h1_*` ×6, `h3_h2_stream_e2e`, `h2_proxy_e2e`, `h1_proxy_e2e`, `h3_h3_stream_e2e` all green; **204 "ok" result lines, 0 FAILED anywhere**; count rose 1132→1155 (nothing removed) |

Aggregate pass count rose from the S6 baseline (1132) to **1155** (+23 = the new
H3→H3 cases). Zero failures, zero new ignores. **GATE: PASS.**

---

## 6. Verdict & S8 handoff

### Verdict — **SESSION 7 COMPLETE**
Exit condition (a) satisfied: Phase 0 green basis + **H3→H3 built and verified**
to the bar + **H2→H1 honest-stopped with its R8 plan authored** + **Phase 3
green**. Promoted `feature/h-matrix-s7` → `main` (`--no-ff`, R11).

### Matrix status after S7 — 3 of 9 BUILT
**BUILT**: H3→H1, H3→H2, **H3→H3 (new this session)**.
**PARTIAL (proxy-on-wire but buffer)**: H2→H1, H1→H1, H1→H2, H2→H2, H1→H3, H2→H3.

### Remaining cells — dependency-ordered build plan for S8+
| Order | Cell | Plane | Size | Net-new connector | Depends on |
|---|---|---|---|---|---|
| 1 | **H2→H1** | lb-l7 | M | **M-D** bounded H2 ingress + validate-before-forward | plan drafted (`s8-h2h1-plan.md`); reuses verified H1 egress |
| 2 | **H1→H1** | lb-l7 | S–M | M-D-lite + **M-E** (R8 gauge + stalled-client mem test) | M-D pattern from #1; body already streams via hyper |
| 3 | **H1→H2** | lb-l7 | M | — | M-B (built in H3→H2) + M-D/M-E |
| 4 | **H2→H2** | lb-l7 | M | — | M-B + M-D/M-E |
| 5 | **H1→H3** | lb-l7→quic | M | — | M-C (built in H3→H3) + M-D/M-E |
| 6 | **H2→H3** | lb-l7→quic | M | — | M-C + M-D/M-E |

Sequencing logic: build **M-D** first (H2→H1) — the highest-value lb-l7-plane
connector (H1 egress already verified); then H1→H1 templates the gauge/test
pattern cheaply; the H?→H2 pair reuses M-B; the H?→H3 pair (last) stacks the
heaviest connector M-C. Both upstream connectors M-B and M-C are now BUILT and
verified, so the remaining six cells are ingress-side (M-D/M-E) + reuse work.

### Program-level remaining work (honest, beyond the matrix)
- **6 of 9 matrix cells still PARTIAL** (above).
- **Chaos/soak suite**: still unbuilt.
- **Native QUIC proxy**: not built — the H3 cells use a quiche-direct bounded
  connector (M-C), not a native QUIC-to-QUIC proxy.
- **WS / gRPC-over-H3**: not built.
- **Full h3spec conformance**: not done (S7 covered targeted RFC 9114 arms:
  trailers, unknown-frame skip, oversized-block reject — not the full suite).
- **F-ESC-1** (multi-kernel verifier CI lane, ~0.5 d) and **N-1** (jumbo-MTU
  `xdp.frags` doc) still open.
- **CF-COV-S7**: H3→H3 session-code headroom is 1 line (§2.4) — fragile to
  future src edits in `h3_to_h3_stream_resp`.

### S8 first actions
1. Re-confirm base = `main` after the S7 promote.
2. Builder refines `s8-h2h1-plan.md` against the live tree; answer Q-D1..Q-D4;
   **lead R8-approval before any source change**.
3. Build H2→H1 to the BUILT bar (§5 of the plan); author≠verifier strict.
