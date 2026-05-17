# SESSION 5 / Phase 0 task #6 — VERIFIER VERDICT

Worktree: /home/ubuntu/Code/eg-verifier (detached @ bd2e6dca, own target/).
Product under test: P1-A/P1-A.1/P1-B committed by S4 builders (bd2e6dca).
Author ≠ verifier upheld: verifier authored/ran the real-wire tests +
proofs; did NOT patch product code.

File: crates/lb-quic/tests/h3_h1_resp_stream_e2e.rs (real
QuicListener → router → conn_actor → h3_bridge::stream_h1_response →
real H1 backend; bodies carry non-UTF-8 0xFF 0x00 0x80).

## Per-item result

| Item | Verdict | Evidence |
|---|---|---|
| R1 | PASS | `#[ignore]` removed; multi-DATA 120 KiB binary body byte-identical at the real H3 client, clean FIN. 3× deterministic. |
| R2 | PASS | Non-vacuous memory bound. See authoritative numbers below. 3× deterministic, identical (max_retained=73 859 B every run). |
| R3 | PASS | Backpressure proven: 4 MiB body, slow client, NO backend stall — gauge stays 73 859 B (≪ ceiling, body-size independent) ⇒ the upstream socket read provably paused (a non-backpressured proxy would retain ≈4 MiB). 3× deterministic. |
| R4 | PASS | `#[ignore]` removed; empty body + zero-length DATA, clean FIN, body empty. 3× deterministic. |
| R5 | PASS | All three abort sub-cases (hard TCP RST, premature-EOF-before-Content-Length, over-cap) ⇒ client observes RESET_STREAM `error_code == H3_INTERNAL_ERROR == 0x0102 != H3_NO_ERROR (0x0100)`, never FIN, no truncated body presented complete. 3× deterministic. |
| R6 | **DEFECT** | Client cancel of the H3 response stream is NOT propagated to stop the upstream read. Proven mechanism — see DEFECT file. Test stays as failing regression lock. |
| R7 | PASS | `#[ignore]` removed; chunked upstream (split sizes 1,7,4096,8192,1,100,99999) reassembled byte-identical via the net-new decoder, clean FIN. 3× deterministic. |
| R8 | PASS (no-regression) | `h3_h1_trailers_resp_e2e.rs` pc1 + pc2 GREEN unchanged (PROTO-2-12). The in-file R8 placeholder stays `#[ignore]`d (P1-C scope, as scaffolded — not a working test disabled). |
| C2 | PARTIAL | 5 of 6 variants PASS (UpstreamReset, PrematureEof, ChunkedDecode, OverCap, BadHead): each ⇒ poisoned upstream NOT parked (single-slot pool `idle_count_for==0`, mirrors `lb_io::pool::non_reusable_is_not_parked`) AND client saw RESET_STREAM 0x0102≠0x0100. 3× deterministic. The 6th, ClientGone, is the SAME defect as R6 — separate failing regression-lock test `c2_clientgone_drops_pooled_upstream`. |
| C3 | PASS | Decoder unit cases GREEN (`chunk_decoder_rejects_malformed_framing_c3`). Added end-to-end real-wire C3: non-hex chunk-size, missing-CRLF-after-chunk, declared-size-overflow, junk-after-zero-terminator — each ⇒ RESET_STREAM 0x0102, never a truncated/forwarded body presented complete. 3× deterministic. |

## Authoritative R2 ceiling numbers (verifier-owned)

Real crate consts: `H3_RESP_CHANNEL_DEPTH=8`, `H3_RESP_CHUNK_MAX=8192`,
`H3_FRAME_HDR_MAX=16` (= `MAX_FRAME_HEADER_BYTES`).

- §1.5 C5 sound channel bound = 8 × (8192 + 16) = **65 664 B**
  (the chunk+hdr form, NOT the looser depth×chunk=65 536 — the gauge
  in `conn_actor.rs:382-385` feeds exactly `used × (CHUNK_MAX +
  FRAME_HDR_MAX)`; the in-file ceiling equals this exactly, asserted
  in-test: `ceiling == 4 * c5_channel_bound`, `c5_channel_bound ==
  65_664`).
- R2 ceiling (×4 slack: one queued StreamTx chunk + full channel + one
  in-flight producer chunk + HEADERS frame) = 4 × 65 664 =
  **262 656 B** (~256.5 KiB).
- Body = 4 MiB = 4 194 304 B ⇒ ceiling≪body margin = 4 194 304 /
  262 656 = **15.97×** (≥8× requirement met; asserted in-test
  `ceiling*8 <= total_body`).
- Empirically observed peak `MAX_RETAINED_RESP_BYTES` = **73 859 B**
  (constant across all 3 R2 + 3 R3 runs). 73 859 / 262 656 = 0.28 of
  the ceiling; 73 859 / 4 194 304 = **1.76 % of the body** — strongly
  non-vacuous and body-size independent (the buffering-trap value
  would be ≈ 4 194 304 B).

## R3 backpressure evidence (explicit)

R3 uses a 4 MiB Content-Length backend with NO backend stall (the
upstream will firehose as fast as TCP allows) and a client that does
NOT read the response stream for 2 s. Mid-stall sample =
**73 859 B**, peak = **73 859 B**, ceiling = 262 656 B, body =
4 194 304 B (3× identical). The ONLY way retained bytes stay 16× below
the body for a firehosing upstream + non-reading client is that
`stream_h1_response`'s `tx.send().await` blocked on the full bounded
channel ⇒ the upstream `stream.read()` was not called ⇒ TCP
backpressure to the upstream. Proven.

## Defect (1) — see DEFECT-clientgone-resp-stream-not-propagated.md

Tier: correctness + resource-exhaustion (security-adjacent). Binding
C2 + §1.3.4 ClientGone violated. The actor has NO path that observes a
client STOP_SENDING/RESET on the *response* stream for a bodyless
request; `resp_rx_by_stream[sid]` is removed nowhere on client-cancel;
`drain_streams_to_conn`'s Progressive branch swallows
`Err(StreamStopped)` at `conn_actor.rs:519` (and when the queue is
empty, `stream_send` is not even called). Producer reads upstream
forever; pooled conn never released. Regression-locked by R6 +
`c2_clientgone_drops_pooled_upstream` (both deterministically FAIL with
the mechanism in the assert message). Verifier did NOT patch (author ≠
verifier) — routed to lead/builder; re-verify after fix.

## Determinism

All passing new/touched tests run 3× with identical results (numbers
above byte-identical run-to-run). The two failing tests fail
deterministically 3× with the same proven mechanism (a real
no-teardown, NOT a scheduling timeout: the endless backend keeps being
writable and its read half never closes — server-side misbehavior is
present, so per the R2 flake protocol this is a REAL DEFECT, not
environmental).

## Workspace regression

`cargo test --workspace --all-features`: the ONLY failures workspace-
wide are `r6_*` and `c2_clientgone_*` (the two defect regression
locks). h3_h1_resp_stream_e2e binary: 13 passed / 2 failed (defect
locks) / 1 ignored (R8 P1-C placeholder). No other crate/test
regressed. R8 trailers pc1/pc2 GREEN. C3 decoder unit GREEN.

## VERDICT

**TASK #6 — DEFECTS: client-cancel of the H3 response stream is not
propagated (R6 + C2/ClientGone) — correctness + resource-exhaustion;
binding C2 + §1.3.4 ClientGone.** All other items (R1, R2, R3, R4, R5,
R7, R8 no-regression, C2 five working variants, C3) VERIFIED PASS with
the proofs/numbers above.
