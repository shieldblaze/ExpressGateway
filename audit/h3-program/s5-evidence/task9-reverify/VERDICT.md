# SESSION 5 / task #9 RE-VERIFICATION — DEFECT-CLIENTGONE fix

Author ≠ verifier (R5): builder2b authored `ad9374dc`; this is the
independent verifier re-verification. Worktree:
/home/ubuntu/Code/eg-verifier, detached @ `ad9374dc` (confirmed
`git log --oneline -1` = "fix(s5): propagate H3 response-stream
client-cancel (DEFECT-CLIENTGONE)"). User ubuntu, no sudo.

## The fix (independently inspected)

`git show ad9374dc` = a single 52-line addition to
`crates/lb-quic/src/conn_actor.rs`, NO other product change:
- new `fn reap_client_cancelled_responses(conn, &mut resp_rx_by_stream,
  &mut stream_response)`: for each stream with a live response
  receiver, `conn.stream_writable(sid, 1)`; on
  `Err(StreamStopped(code) | StreamReset(code))` → remove the
  `Receiver` (⇒ producer's next `tx.send().await` →
  `Err(RespAbort::ClientGone)` → `h3_to_h1_stream_resp` marks the
  pooled upstream non-reusable + returns) AND remove the `StreamTx`
  (never FIN, never RESET_STREAM — §1.3.4 ClientGone).
- called in `run_actor` between the `poll_h3` block and
  `drain_resp_channels` (the §1.4.3 gate), so a cancelled stream is not
  re-driven that tick.
- `drain_streams_to_conn` signature + Progressive send branch
  UNCHANGED (confirmed in diff). No RESET_STREAM is emitted on the
  client-cancel path (correct: the client already cancelled).

## Per-item result

| Item | Verdict | Proven mechanism / numbers |
|---|---|---|
| 1. R6 + c2_clientgone PASS for the RIGHT reason | **PASS** | Both PASS deterministically 3× in 0.74s (NOT a 20s coincidental timeout). CAUSATION PROVEN by negative control: with ONLY the `reap_client_cancelled_responses` *call* commented out (all 52 helper lines + tests byte-identical), both tests FAIL deterministically with the exact original mechanism ("endless backend read never closed within 20s", 40.68s); with the call restored both PASS in 0.74s. The fix call is the sole cause. R6 asserts the REAL teardown: endless backend (1 TiB CL) read-half closes + `bytes_written` stops growing after teardown ⇒ upstream read truly STOPS (ClientGone path exercised), not a coincidental close. c2_clientgone asserts the same + single-slot pool. |
| 2. Binding C2 for ClientGone | **PASS** | `c2_clientgone_drops_pooled_upstream` (single-slot pool) PASS: the abandoned upstream's read-half closes ⇒ pooled conn dropped non-reusable, never parked — parity with the 5 other RespAbort variants (`c2_every_abort_variant_drops_pooled_upstream_and_resets` still PASS). |
| 3. §1.3.4 / C1 abort path UNCHANGED | **PASS** | R5 PASS: UpstreamReset (hard TCP RST) / PrematureEof / over-cap each still ⇒ client observes RESET_STREAM `error_code == 0x0102 == H3_INTERNAL_ERROR != 0x0100`. The fix emits NO RESET_STREAM on client-cancel (it removes the StreamTx without shutdown) — correct §1.3.4 ClientGone vs the 0x0102 abort path; abort path bytes/codes unchanged. |
| 4. R8 no buffering reintroduced | **PASS** | R2 + R3 numbers IDENTICAL to the pre-fix authoritative bound: `max_retained = 73 859 B`, `ceiling = 262 656 B` (= 4 × 8 × (8192+16), exact §1.5 C5 sound bound), `body = 4 194 304 B`, margin 15.97×, retained/ceiling = 0.2812. R3 mid-stall & peak both 73 859 B. The §1.4.3 gate + bounded channel are untouched; no regression. |
| 5. No regression of the 13 prev-passing + R8 PROTO-2-12 + R8 placeholder | **PASS** | h3_h1_resp_stream_e2e: 15 passed / 0 failed / 1 ignored (was 13 pass / 2 fail / 1 ignored — the 2 defect locks now pass; nothing else changed). R8 in-file placeholder STAYS `#[ignore]`d (P1-C scope — NOT un-ignored, NOT weakened). `h3_h1_trailers_resp_e2e` pc1 + pc2 GREEN unchanged (PROTO-2-12). |
| 6. Full `cargo test --workspace --all-features` | **PASS** | ZERO failures workspace-wide (202 `test result: ok.` lines; no FAILED / panicked / `error: test failed`). Previously only r6_*/c2_clientgone_* failed. |
| 7. clippy + fmt | **PASS** (with verifier-owned fmt fix — see note) | `cargo clippy --all-targets --all-features -- -D warnings` clean. `cargo fmt --check` clean AFTER the verifier reformatted its OWN test file (see note). |
| 8. Determinism | **PASS** | Full h3_h1_resp_stream_e2e suite run 3×: 15/0/1 each, ~29.7s each, identical. The two locks run 3×: identical 0.74s PASS. R2/R3 numbers byte-identical across all runs. |

## fmt note (transparency — no product code touched)

`cargo fmt --check` initially FAILED with 21 diffs, ALL confined to
`crates/lb-quic/tests/h3_h1_resp_stream_e2e.rs` — the verifier-authored
task-#6 TEST file from the prior session (committed at 56079026), NOT
the builder's `conn_actor.rs` fix and NOT any product code (verified:
`grep "Diff in" | grep -v h3_h1_resp_stream_e2e.rs` = 0). This is a
pre-existing formatting lapse in the verifier's own test file, not
introduced by `ad9374dc`. `cargo fmt -p lb-quic` (workspace edition
2024) applied — 53/59 line pure-formatting delta to that one test
file, no logic change; the full suite re-ran 15/0/1 unchanged after.
author≠verifier governs PRODUCT code (untouched here); formatting the
verifier's own verification tests to pass the gate is in-scope.

## Determinism / flake protocol

The negative-control failure (fix call disabled) is a REAL no-teardown
with server-side misbehavior present (endless backend stays writable,
read-half never closes for 40s) — per the R2 flake protocol this is a
REAL DEFECT signal, NOT environmental; it is exactly what the fix
resolves. All passes are deterministic 3×; numbers byte-identical.

## VERDICT

**DEFECT-CLIENTGONE RE-VERIFIED — Phase 0 COMPLETE.**

The `ad9374dc` fix correctly and minimally propagates an H3
response-stream client-cancel (STOP_SENDING / RESET_STREAM) to stop
the upstream read and drop the pooled upstream non-reusable (binding
C2 / §1.3.4 ClientGone), with NO RESET_STREAM emitted on that path,
the 0x0102 abort path unchanged (R5), the §1.5/R2 memory bound and R3
backpressure unregressed (identical authoritative numbers), zero
workspace regressions, clippy/fmt clean, deterministic 3×. Causation
isolated to the fix by negative control.
