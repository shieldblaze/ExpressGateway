# Task #15 (i) — J5-FIX SRC VERIFICATION (independent, STRICT,
# never by inference)

Verifier: `verifier` (R5 — builder-1 authored J1 src + the fix;
verifier≠builder-1). Target: s7/builder-1 @ fedb5cf4
("fix(F-S7-2/J5): discriminate benign stream-collected vs genuine
reset in H3→H3 recv-half"), parent e42a9b4e (committed J4 asset),
linear FF no force. quiche 0.28.0. All runs: detached @ fedb5cf4 in
s7-verifier, CARGO_TARGET_DIR exported; measurement probes restored
byte-identical (sha1 7971d3f8…, git clean) — R5 honored.

## VERDICT: PARTIAL PASS — F-S7-2 GENUINELY FIXED; cases 4,5 still
## FAIL (NOT a fix regression; root = F-S7-4 harness + a NEWLY-
## UNCOVERED original-J1 src defect F-S7-6). H3→H3 cell NOT BUILT.

### STRUCTURE / GATES
* J5-G3 scope — PASS. `git diff --stat e42a9b4e fedb5cf4` = ONLY
  crates/lb-quic/src/h3_bridge.rs (+132 −6); exactly 3 hunks: the
  recv-arm @3039, the `classify_recv_err`+`RecvErrClass` classifier
  @3484, the pure `s7_j5_recv_stream_err_classification` test @4468.
  Nothing outside recv arm + classifier + pure test. No other file.
* J5-G2 — PASS. `classify_recv_err` is factored module-level; the
  production recv arm calls EXACTLY it; the unit test exercises the
  same fn (not a re-statement).
* J5-G1 (quiche 0.28.0 variant ids) — PASS, verified vs
  quiche-0.28.0/src/error.rs: `Done` (unit :39),
  `InvalidStreamState(u64)` (:62), `StreamLimit` (unit :77),
  `StreamStopped(u64)` (:83), `StreamReset(u64)` (:89),
  `FinalSize` (unit :92). The fix `match e { Done => Done,
  StreamReset(_)|FinalSize => GenuineReset, _ => BenignCollected }`
  — ids + arities exactly correct.
* lib — PASS: `cargo test -p lb-quic --lib --features test-gauges`
  = 26 passed / 0 / 0, incl. `s7_j5_recv_stream_err_classification`
  + s7_j1 + s7_j2 all ok.
* Regression — PASS: h3_h2_stream_e2e 10/10; h3_h1_stream_body_e2e
  6/6; h3_h1_resp_stream_e2e 16/16; round8_h3_authority_enforced
  3/3.
* Corrected clippy `-p lb-quic --all-targets --features
  test-gauges -- -D warnings` — CLEAN (exit 0).
* fmt: J5-FIX SRC (h3_bridge.rs/conn_actor.rs) CLEAN. The only fmt
  diffs are in the J4 TEST asset h3_h3_stream_e2e.rs = the
  separately-tracked F-S7-5, NOT J5-FIX src.

### GENUINE J4 SUITE ON THE FIX (the binding proof, never inferred)
`--test-threads=1`, --features test-gauges, fedb5cf4:
  case 1 h3h3_e2e_get_response_byte_identical .............. ok
  case 2 h3h3_e2e_request_body_byte_identical_at_backend ... ok
  case 3 h3h3_e2e_response_memory_bounded_through_
         stalled_client ................................... ok
  case 6 h3h3_e2e_upstream_reset_midbody_resets_client_
         no_fin ........................................... ok
  case 7 h3h3_e2e_client_reset_midrequest_rsts_upstream_
         no_truncated_request ............................. ok
  case 4 h3h3_e2e_request_memory_bounded_through_
         stalled_backend .................................. FAILED
  case 5 h3h3_e2e_backpressure_stalled_client_pauses_
         upstream_read .................................... FAILED
  ⇒ 5 passed / 2 failed.

* R-FIX-1 (relay the REAL response, not synthesized) — PASS.
  Case 1 asserts real 200 + sentinel body byte-identical; case 2
  asserts a ≥1 MiB NON-UTF-8 BINARY request body byte-identical AT
  THE REAL quiche backend + trailers-dropped — BOTH ok. A
  synthesized/empty "fixed" response would FAIL these; they pass ⇒
  the fix relays the genuine upstream status/headers/body. Case 3
  (4 MiB response through a stalled client) ok ⇒ real large body
  relayed byte-identical.
* R-FIX-2 (smuggling / response-splitting guard NOT regressed) —
  PASS. Case 6 (upstream RESET mid-response ⇒ no clean 200) ok;
  case 7 (client RESET mid-request ⇒ upstream no truncated-as-
  complete) ok. `GenuineReset` (StreamReset/FinalSize) still ⇒
  `UpstreamReset`, never End-on-partial. The classifier does NOT
  blanket-map Err⇒done.
* R-FIX-3 (R8 classification-only, no retention) — PASS. The fix
  adds only an error-classification enum + match; no buffer, no
  retention; the bounded-incremental window is unchanged (cases
  1/2/3 byte-identity holds; my (ii) probes show retained ≤ ~74 KB
  for 4 MiB bodies, body-size-independent).

### CASES 4 & 5 — PROVEN MECHANISM (R2; instrumented, not inferred)
Post-loop probe on fedb5cf4:
* Case 5: `response_complete=false outcome_ok=true sent_head=true
  elapsed_ms=5000 total_relayed=4369186` of 8 388 608 (8 MiB). The
  recv-half relayed ~4.37 MiB then hit the J1 recv-half's HARDCODED
  `Duration::from_secs(5)` wall-clock deadline (h3_bridge.rs:2911)
  and aborted mid-body ⇒ `PrematureEof`, no clean FIN. The
  proven-correct sibling `request_h3_upstream` uses 30 s
  (h3_bridge.rs:2542); the BUILT H3→H2 `stream_h2_response` has NO
  fixed wall-clock cap (event-driven). ⇒ a GENUINE SRC DEFECT
  in the ORIGINAL J1 recv-half: an arbitrary 5 s wall-clock
  deadline truncates a legitimate, actively-progressing large
  streamed response under normal backpressure. NOT introduced by
  J5-FIX (the deadline is pre-existing J1 code) but UNCOVERED now
  that F-S7-2 no longer masks the path. Recommend **F-S7-6**.
* Case 4: `response_complete=false outcome_ok=true sent_head=false
  elapsed_ms=5003 total_relayed=0`. Zero response bytes in ~5 s —
  the upstream never produced a response in time. Root = the
  F-S7-4 harness defect (deliverable ii: `StallReadThenEcho`'s
  stall is ineffective — never engages the J2 `stream_capacity`
  gate; the upstream only emits its response after `req_fin`, and
  the 4 MiB request transfer through the broken-stall harness +
  the same 5 s recv cap never completes the cycle). Compounded by
  the F-S7-6 5 s deadline. NOT a J5-FIX regression.

### NET
J5-FIX **genuinely and correctly fixes F-S7-2** (cases 1/2/3/6/7
pass with byte-identity on the genuine wire; guards intact; scope/
gates/lib/regression/clippy all green; quiche ids correct). It is
sound and should be accepted AS THE F-S7-2 fix. BUT the H3→H3 cell
is **NOT BUILT**: cases 4 & 5 (the R8 memory/backpressure proofs)
still fail, for TWO further issues my mechanism proofs localize:
  * **F-S7-6 (NEW src defect, recommend open)** — J1 recv-half
    hardcoded 5 s wall-clock deadline truncates valid large/slow
    streamed responses (case 5 proof: 4.37/8 MiB at exactly
    5000 ms). R6 input: TRACTABLE (localized constant/while-loop
    condition in h3_to_h3_stream_resp; mirror request_h3_upstream's
    30 s OR make it idle-based not total-wall-clock — severity is
    lead's call).
  * **F-S7-4 (harness defect, recommend open)** — J4 case-4 stall
    ineffective (deliverable ii, independently proven): never
    engages the J2 `stream_capacity` gate ⇒ case 4 cannot prove
    request-side backpressure and currently can't even complete.
    TEST defect. R6: TRACTABLE (localized J4 upstream change:
    actually stop `stream_recv` during the stall window).
The gateway request- AND response-side memory bounds are
INDEPENDENTLY PROVEN SOUND (deliverable ii: ≤~74 KB retained for
4 MiB bodies, body-size-independent) — there is NO latent src
memory defect; the remaining src issue is solely the F-S7-6
deadline. Cell BUILT requires F-S7-2(J5)+F-S7-6 fixed AND a
corrected case-4 harness, with cases 1-5 genuinely (non-vacuously)
green and 6/7 still guarding.
