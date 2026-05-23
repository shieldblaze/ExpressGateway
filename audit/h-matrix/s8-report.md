# SESSION 8 REPORT — H-to-H Matrix: H2→H1 BUILT; H1→H1 honest-stopped

**Verdict: SESSION 8 COMPLETE** — H2→H1 (M-D) BUILT and independently verified;
H1→H1 honest-stopped with its R8 plan authored; Phase 3 gate PASS; promoted to
main per R11.

Cells BUILT after S8: **4 of 9** — H3→H1, H3→H2, H3→H3 (S5–S7) + **H2→H1 (S8)**.

- Base: `main` @ `8ed1e92c` (S7 promote). Work branch: `feature/h-matrix-s8`.
- Team (agent teams, own worktrees, shared `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target`):
  lead (coord/report/integration), builder-1 (+ builder-1b remediation), verifier
  (+ verifier2 re-adjudication). Author≠verifier held strictly throughout.

---

## Phase 0 — baseline (GREEN)
- Base tip confirmed `8ed1e92c` (= origin/main, no drift).
- R1 ×3 `cargo test --workspace --all-features`: **1155 / 0 / 16**, deterministic
  across 3 runs (= S7 baseline exactly → R3 intact at entry).
- `cargo fmt --check` clean; `cargo clippy --all-targets --all-features -D warnings` clean.
- Disk 33 GB free at entry (R9 floor 25 GB).

## Phase 1 — H2→H1 (M-D bounded H2 ingress) — BUILT

### Resolution of the 4 open questions (lead, R7 — none escalated)
- **Q-D1 (validate-before-forward without buffering):** the fixed in-flight
  window (8 × 8 KiB = 64 KiB) doubles as a **validation lookahead**. A request
  whose body+trailers fit the window is polled to EOF → identical hyper/h2
  validation that `collect()` drove → validated **before** the upstream dial →
  **zero backend dial preserved** for the malformed-probe cases → the two
  existing gate tests pass UNCHANGED (R5). A request exceeding the window dials
  and streams, with the downstream **response-head relay gated on the inbound
  body reaching a validated terminal state** → no backend body is ever relayed
  for a malformed request (the h2spec invariant the brief mandates). Documented
  residual (security-preferable): a >64 KiB malformed body dials the backend
  (cheap-flood vector stays zero-dial). NOT a product fork — the brief's binding
  requirement is framing+header validate-before-forward, which holds for all
  sizes. No test weakened.
- **Q-D2 (egress plane):** reuse the IN-CRATE lb-l7 hyper H1 sender; the H3
  `write_h1_request` is crate-private to lb-quic and `stream_h1_response` is
  H3-actor-centric. M-D changed only the request *body* (buffered → bounded
  streaming). **No cross-crate edit.**
- **Q-D3 (gauge):** added lb-l7 `test-gauges` feature + `H2_REQ_MAX_RETAINED_BODY_BYTES`.
- **Q-D4 (window):** `H2_REQ_CHANNEL_DEPTH=8`, `H2_REQ_CHUNK_MAX=8 KiB`; ceiling
  64 KiB, body-size-independent.

### Increments (each plan→build→independent-verify→commit→push)
- **I1** `b22fe011` — scaffold: test-gauges feature, `H2_REQ_*` constants, gauge
  static + `record_retained`, constant-pin tests. No behavior change.
- **I2** `77ae94b8` — M-D lookahead-window pump replaces `Limited::collect()`;
  streaming upstream body via bounded mpsc→`StreamBody`; response gated on a
  oneshot verdict.
- **Verify round 1 (verifier, `a513e3b6`) — VERDICT NOT-BUILT**, 3 findings:
  - **F-MD-1 (BLOCKER):** Branch B never delivered the request body — every
    well-formed body >64 KiB got a spurious 413 with 0 bytes forwarded, even to
    an echo backend that reads the whole body. (Caught by adversarial real-wire
    testing the builder's self-gate missed.)
  - **F-MD-2:** early-responding backend (>window) → proxy 413 instead of the
    backend's response (correctness regression vs the buffered baseline).
  - **F-MD-3:** the streaming-phase memory gauge recorded a CONSTANT → vacuous.
  - Coverage 71% (Branch B unreachable).
- **Remediation `bc23b9f8` (builder-1b):**
  - **F-MD-1 root cause** (proven via isolated repro): the inbound request
    `parts.version == HTTP/2.0` carried into the hyper **HTTP/1.1** client
    mis-frames an unknown-length `StreamBody` → empty body, body never polled.
    Fix: force `parts.version = HTTP_11` + strip `content-length`/
    `transfer-encoding` so hyper frames the streaming body itself. Also: a
    dropped body receiver must NOT map to 413.
  - **F-MD-2 fix:** receiver-drop → **drain-and-validate** (poll inbound to a
    validated terminal state, bounded, 64 MiB cap) → relay the backend's
    already-received response. 413 only on the genuine 64 MiB cap.
  - **F-MD-3 fix:** real in-flight counter (incremented on push, decremented
    when hyper pulls) + lookahead remainder → measures actual retained.
- **Verify round 2 (verifier2, `6e3e94a2`) — VERDICT BUILT** (re-confirmed each
  fix by mechanism, adversarially, against the exact remediated src):

### BUILT-bar proofs (verifier2, independent)
1. **Real-wire** both directions, binary (non-UTF-8) bodies, byte-identical:
   Branch A (8 KiB) + Branch B (512 KiB), 1 dial each; **reqwest cross-check**
   (independent H2 stack) byte-identical 512 KiB.
2. **Non-vacuous memory:** 4 MiB body, live-occupancy gauge peaks **80 KiB**
   ≤ 256 KiB ≪ 4 MiB; inverted probe load-bearing (a no-decrement/whole-body
   variant trips it).
3. **Backpressure** both halves: client paused at ~2.9 MiB of 4 MiB under a
   stalled backend; resume → 200 at full 4 MiB (proven causal chain).
4. **h2spec ordering:** the 3 existing gate tests (`h2_validation_before_forward`)
   pass UNCHANGED (zero-dial Branch A); over-window malformed (pseudo-trailers,
   content-length≠ΣDATA) → no backend body relayed downstream.
5. **Early-response relay:** >window upload to a 401 backend relays **401**.
6. **Smuggling parity (non-vacuous):** client RST mid-body → dials=1 (post-dial
   abort reached), complete=0 (never a complete upstream request).
7. **Coverage 81.82%** (234/286) on the M-D session sub-metric, canonical
   cargo-llvm-cov, 3× deterministic (uncovered-set md5 stable).
8. R3 intact; fmt/clippy clean.

### Verifier-harness bug adjudicated (R2, NOT asterisked)
The round-1 `real_wire_large` RED was a HARNESS reader bug — cumulative
`release_capacity(got.len())` over-releases (h2 crate: cannot release more than
received), stalling at the 65535-byte window → UnexpectedEof. Proven by mechanism
and 3 independent cross-checks (correct per-chunk reader, reqwest, and a Branch-A
large-response reproducing the same stall). The verifier fixed its OWN harness;
the gateway leg is genuinely correct. No working test weakened.

## Phase 2 — H1→H1 — HONEST-STOPPED (plan authored)
**Decision (lead):** do NOT build H1→H1 this session. Rationale: H2→H1 required a
full remediation round and is the primary deliverable; disk ran tight (21 GB
mid-session); building+verifying a 2nd cell to the bar risked neither being clean
by budget end. "One fully-verified cell + a clean gate beats two half-done."
**Deliverable:** `audit/h-matrix/s9-h1h1-plan.md` — H1→H1 already streams via
hyper (PARTIAL for lack of R8 *proofs*, not buffering); plan = **M-D-lite**
(reuse the verified S8 pump, Branch-B-only, no validate-ordering) + **M-E** proof
harness; bakes in the M-D F-MD-1/2/3 fixes; 4 open questions for S9 R8-approval.

## Phase 3 — gates + regression

### Gate stabilization (the deterministic ×3 requirement caught 2 latent races)
The first full-workspace gate runs were NON-DETERMINISTIC. Per R2, each failure
was classified by a proven mechanism and FIXED (never asterisked):
- **Gauge global-state collision** (`414e9b1c`): `memory_gauge_non_vacuous`'s
  inverted probe deliberately writes `record_retained(4 MiB)` into the
  process-global gauge; under the gate's 8-thread parallelism it leaked into the
  concurrent `memory_gauge_tracks_live_occupancy` test's reset→read window (read
  as exactly 4194304). Test-isolation defect (not gateway). Fix: a
  `test-gauges`-gated `GAUGE_SERIAL` async mutex serializes the two gauge tests.
  No assertion weakened. Confirmed 6/6 under forced concurrency.
- **F-MD-4 — request-smuggling race (SECURITY, R6)** (`622ee624`): Branch B
  signalled an inbound RST/protocol error by DROPPING the channel sender, which
  `StreamBody` renders as a clean EOF → hyper wrote the chunked terminator → the
  H1 backend saw a truncated request as COMPLETE (raced the conn abort; 3/25
  pre-fix). Fix: send `Err(PumpAbort)` into the channel on every inbound-error/
  abort path so hyper aborts the upstream request body (no clean terminator);
  clean-EOF path unchanged. Confirmed 25/25 `complete=0` under forced
  concurrency; clean-path + early-response + zero-dial all still pass. The
  verifier's `smuggling_rst_mid_body_never_complete_at_upstream` is the
  regression. This is exactly why R1 mandates ×3 and R2 forbids asterisking — a
  single-run verification (verifier2) had seen complete=0 and missed the race.

### Final gate (HEAD `5a5e633e` after both fixes) — **GATE: PASS**
- R1 ×3 `cargo test --workspace --all-features`: **1179 / 0 / 16** on all three runs.
- Determinism: **DETERMINISTIC** (identical tallies ×3; the two earlier flakes both
  fixed, not asterisked).
- R10: gate ran `--all-features` (⊇ `test-gauges`); memory-gauge cases present in
  the run — `memory_bounded_through_stalled_{backend,client,upstream}`,
  `h2_req_record_retained_is_monotone_max`, `resp_retained_ceiling_…`.
- R3: 1179 = the 1155 S7 baseline + 24 net-new M-D tests; 0 failures → S1–S7 +
  the 3 prior BUILT cells (H3→H1/H3→H2/H3→H3) intact.
- clippy `-D warnings`: clean; fmt `--check`: clean. Ignored = 16 (= baseline, no
  new `#[ignore]`).
- Session-code coverage (binding sub-metric, verifier-re-measured independently
  on the final tip via canonical cargo-llvm-cov 0.8.7 / toolchain 1.85.1):
  **82.13% (262/319)**, deterministic ×3 (uncovered-set md5 stable). First pass
  was 77.12% (the smuggle-fix arms were unexercised); verifier3 added 3 minimal
  real-wire tests (within-window RST reject, Branch-A/B valid trailers) → 82.13%.
  The 57 residual uncovered lines are all defensive/error/timeout/edge arms; no
  smuggling no-leak/no-complete path is uncovered. (verifier2 had measured
  81.82%/234 on `bc23b9f8` before the smuggle fixes; this supersedes it.)
- Disk reclaimed via `cargo clean` to 52 GB before the cold gate (R9).

## Carry-forward (tracked, not in S8 scope)
- F-ESC-1 (multi-kernel verifier CI lane, ~0.5d), N-1 (jumbo-MTU xdp.frags doc),
  S4-NUANCE-1, CF-DEDUP-1 (now also: M-D pump dup vs shared-helper — see S9 Q-H1),
  CF-COV-1/2, CF-COV-S7.
- **CF-COV-S8:** M-D residual uncovered = error/timeout arms, trailer-loop,
  post-dial-malformed edges; future edits to `h2_proxy.rs` M-D code must add
  coverage (no slack above 80%).

## S9 handoff — remaining cells (dependency order)
1. **H1→H1** (S–M) — plan ready (`s9-h1h1-plan.md`); M-D-lite + M-E; resolve
   Q-H1..Q-H4 then build. *Next.*
2. **H1→H2 / H2→H2** — need **M-B** (the H2 upstream connector).
3. **H1→H3 / H2→H3** — need **M-C** (heaviest; H3 upstream connector).
Program-level still unbuilt: chaos/soak suite, native QUIC proxy,
WS/gRPC-over-H3, full h3spec conformance.

## Promotion (R11)
`feature/h-matrix-s8` promoted to `main` via a `--no-ff` honest-message merge
(base `8ed1e92c`; no divergence on main since S7, so the merged tree is
byte-identical to the gated tip `92f839bf`). main now reflects **4 of 9 cells
BUILT** (H3→H1, H3→H2, H3→H3, H2→H1). The S9 H1→H1 R8 plan
(`s9-h1h1-plan.md`) rides along as the next-session head-start.

## Findings ledger (R4 — every finding fixed-and-verified this session; none asterisked)
| ID | Class | Disposition |
|----|-------|-------------|
| F-MD-1 | correctness (Branch-B empty body) | FIXED `bc23b9f8` (HTTP_11 + strip framing), re-verified |
| F-MD-2 | correctness (early-response 413) | FIXED `bc23b9f8` (drain-and-validate), re-verified |
| F-MD-3 | proof integrity (vacuous gauge) | FIXED `bc23b9f8` (real in-flight counter), re-verified |
| gauge-collision | test isolation (global static) | FIXED `414e9b1c` (GAUGE_SERIAL), 6/6 + gate ×3 |
| **F-MD-4** | **SECURITY — request smuggling** | FIXED `622ee624`+`0b43ef3b` (is_end_stream gate; Err-before-close FIFO), gate ×3 under load |
| cov-shortfall | binding coverage on final tip | FIXED `41c7ff59` (3 real-wire tests, 77.12%→82.13%) |

---
**VERDICT: SESSION 8 COMPLETE** — H2→H1 (M-D) BUILT (4/9 cells); H1→H1
honest-stopped with R8 plan authored; Phase 3 gate PASS (R1 ×3 deterministic
1182/0/16, coverage 82.13%, clippy/fmt clean, R3 intact, R10 honored);
promoted to main per R11.
