# Task #15 (ii) — R2 PROVEN-MECHANISM src-vs-harness adjudication
# of J4 cases 3, 4, 5

Verifier: `verifier` (R5 — builder-1 authored BOTH J1 src and the J4
harness; self-classification NOT accepted, every verdict here is from
independent reproduction + instrumentation, src restored
byte-identical). Adjudicated against the committed J4 source-of-record
e42a9b4e (src == accepted-J3 d17e51c4, zero src delta). quiche 0.28.0.
Measurement probes inserted in s7-verifier worktree only, then both
h3_bridge.rs (sha1 cf6f60c1…) and conn_actor.rs (sha1 393e3894…)
restored byte-identical (git status clean) — R5 honored.

## CASE 4 — request-side memory/backpressure (CRITICAL) — HARNESS
DEFECT; gateway request-side memory bounding INDEPENDENTLY SOUND
Builder-1 claimed the inline `StallReadThenEcho` upstream still calls
`stream_recv` so the gateway `stream_capacity` backpressure never
engages. I did NOT take that on its word — proven from code + runtime:
* CODE: J4 harness lines 616-631 call `conn.stream_recv(r, ..)`
  UNCONDITIONALLY every loop iteration, draining ALL request bytes
  into an unbounded `rx_tail`. The "stall" (lines 637-667) only gates
  whether `decode_frame` runs on already-drained bytes — it does NOT
  gate `stream_recv`. So QUIC `MAX_STREAM_DATA` credit keeps flowing.
* RUNTIME (instrumented `stream_capacity` gate + the
  `record_retained_for_stream` gauge inputs, 4 MiB POST body):
    - `FS74PROBE stream_capacity==0` count = **0** across the whole
      run — the J2 request-side backpressure gate (`Ok(0)` branch,
      h3_bridge.rs:3013) NEVER fires. Only `LOW = 2700` / `= 111`
      transient dips, never 0. Builder-1's harness-defect claim
      INDEPENDENTLY CONFIRMED.
    - Gauge inputs: `rx_bytes=0` ALWAYS (StreamRxBuf never
      whole-buffers), `pending_bytes` ≤ 8192 (exactly one
      `H3_BODY_CHUNK_MAX`), `chan_used` 1-8 (bounded depth-8
      `body_tx`), **max TOTAL retained = 73728 B** for a 4 194 304-B
      body — ~57× under body, ~3.6× under the 262 656 ceiling,
      BODY-SIZE-INDEPENDENT. 1819 incremental `body_rx` pulls (J2
      drains chunk-by-chunk, no whole-body buffer).
VERDICT case 4: **HARNESS DEFECT (not src)**. The gateway request-side
memory bound is INDEPENDENTLY PROVEN SOUND — the M-A pump's bounded
`body_tx`(depth 8) + `body_pending`(≤1 chunk) + non-buffering
`StreamRxBuf` hold retained memory depth-bounded and body-size
independent EVEN THOUGH the J2 `stream_capacity` gate never had to
fire. NO latent src backpressure/memory defect. BUT: the J4 harness
as written CANNOT prove the J2 `stream_capacity` gate itself works
(it never fires) — so case 4, even after F-S7-2 is fixed, would
VACUOUSLY pass its `retained ≤ ceiling` assertion without exercising
the J2 gate. That is a harness-soundness gap (a correct stall = an
upstream that stops calling `stream_recv` at the transport layer,
not just deferring decode). Recommend opening **F-S7-4 = J4
harness-soundness defect (case-4 stall ineffective)** — a TEST defect,
NOT a src defect. Do NOT mark the H3→H3 cell BUILT on a vacuous
case 4.

## CASE 3 — response-side memory through stalled client — F-S7-2
(same root recv-half defect); response-side memory bound SOUND
Reproduced case 3 in FULL ISOLATION (`--exact`, single test in
process — eliminates the gauge-static cross-test/parallelism
hypothesis): it STILL FAILS, at line 980 `assert!(out.fin, …)` —
NOT line 979 (`status==Some(200)` PASSES). So the "gauge-static
cross-test interference" hypothesis is NOT the primary cause: case 3
fails in isolation due to **F-S7-2**. Mechanism difference vs cases
1/2/4: `LargeResp` streams a 4 MiB response, so HEADERS + early DATA
relay (client sees 200) BEFORE the stream completes; the F-S7-2
recv-half `InvalidStreamState`-on-collected-stream bug then strikes
mid-stream ⇒ client gets the head but never a clean FIN.
Instrumented response gauge (4 MiB `LargeResp`): max
`record_resp_retained` TOTAL = **73 859 B** — ~57× under body, ~3.6×
under the 262 656 ceiling, body-size-independent. The response-side
memory bound (progressive `StreamTx` queue + bounded depth-8
`resp_rx`) is **R8-SOUND**.
VERDICT case 3: fails due to **F-S7-2 (src, the J1 recv-half defect
already diagnosed)**, surfacing as no-clean-FIN not no-status because
the large streamed body lets the head through first. NOT a distinct
defect; NOT primarily gauge cross-test interference (fails isolated).
Gauge-static parallelism could still be a SECONDARY hardening item
(the 5 gauge cases share process-global statics; only `--exact` /
`--test-threads=1` is interference-free) — recommend the J4 suite
serialize the gauge cases or snapshot deltas; a TEST-soundness nit,
not a src defect. Response-side src memory bound proven sound.

## CASE 5 — large-body completion under stalled client — F-S7-2
Case 5 (`…backpressure_stalled_client_pauses_upstream_read`) asserts
`status==Some(200)` + body byte-identity + retained ≤ ceiling for a
body ≫ ceiling. It fails for the SAME F-S7-2 root: the response-leg
recv-half aborts (`InvalidStreamState`→`UpstreamReset`) so the client
never gets a clean relayed 200/body. Same response-side memory
soundness as case 3 (shared writer/gauge; max ~73 859 B). Mechanism =
F-S7-2 (src). NOT a distinct defect.

## CROSS-CUTTING SRC SOUNDNESS CONCLUSION
The H3→H3 memory bounds — BOTH directions — are independently proven
R8-SOUND under multi-MiB load (request ≤73 728 B, response ≤73 859 B
for 4 MiB bodies; body-size independent; no whole-body buffering).
The ONLY src defect blocking cases 1-5 is **F-S7-2** (the J1 recv-half
`InvalidStreamState`→`UpstreamReset` misclassification, already
diagnosed; fix = J5/#14). There is NO additional latent src
backpressure/memory defect. The remaining issues are HARNESS-side:
* F-S7-4 (recommend open): J4 case-4 stall is ineffective — never
  engages the J2 `stream_capacity` gate ⇒ case 4 would vacuously pass.
  TEST defect.
* secondary: gauge-static process-global sharing across the 5 cases
  ⇒ require `--test-threads=1`/`--exact` or per-case snapshot. TEST
  hardening.
R6 tractability INPUT (severity = lead's call): F-S7-4 is a localized
TEST-harness change (make the case-4 upstream genuinely stop
`stream_recv` during the stall window) — TRACTABLE, same class as the
J4 asset itself. No src work implied by cases 3/4/5 beyond F-S7-2/J5.

BINDING: the H3→H3 cell is NOT BUILT until cases 3,4,5 genuinely pass
on a CORRECTED harness against the F-S7-2-fixed src — not asterisked,
not vacuous (esp. case 4 must actually engage the J2 gate).
