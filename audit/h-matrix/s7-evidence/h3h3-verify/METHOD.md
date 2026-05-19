# S7 #7 — H3→H3 Full Cell Independent Verification: PREPARED METHOD

Status: PREPARED (assembled while J4 builds). EXECUTE only after
team-lead accepts J4 and unblocks #7. Author != verifier (R5);
read-only on builder source; CARGO_TARGET_DIR exported to the shared
target; 12G disk floor; --features test-gauges where the gauge cases
require it. Target commit = the accepted J4 tip on s7/builder-1
(fetch + detached checkout; do NOT merge).

BAR = the S6 H3→H2 "BUILT" template (audit/h-matrix/s6-evidence/
h3h2-verify/VERDICT.md), RAISED by two S7-specific deltas:
 * the upstream is a GENUINE quiche H3 endpoint (not a hyper/TCP
   backend) — real Initial/handshake on a real UdpSocket speaking
   lb_h3;
 * independent coverage ≥80% of session code via the canonical
   `cargo llvm-cov` (NOT builder self-measure).

Ceiling formula (authoritative crate consts, confirmed in tree):
  retained_ceiling = 4 × (H3_RESP_CHANNEL_DEPTH ×
                          (H3_RESP_CHUNK_MAX + H3_FRAME_HDR_MAX))
                   = 4 × (8 × (8192 + 16)) = 262_656 B
Same value both directions (BODY consts mirror RESP). #7 spec & S6
BUILT bar both require exactly 262656 with ≥8× body margin.

## CHECK PLAN (each = PASS/FAIL with mechanism, evidence file)

1. REAL-WIRE GENUINE (adversarial). Read the J4 e2e test +
   inlined upstream. CONFIRM the upstream is a real
   `quiche::accept` server bound to a real `UdpSocket`: it must
   receive a real client **Initial**, complete a real QUIC
   handshake, and speak **lb_h3** frames (HEADERS/DATA varint
   framing) — NOT a stub, NOT a loopback shortcut, NOT a
   pre-canned byte script. Trace the path:
   `QuicListener::spawn` (+ `.with_h3_backend`) → `router::spawn`
   → `conn_actor::run_actor`/`poll_h3` (the J3 LIVE h3_backend
   branch) → `h3_to_h3_stream_resp` → genuine quiche upstream.
   Binary (non-UTF-8) request AND response bodies. Evidence:
   realwire-run{1,2,3}.txt + a code-trace note.

2. NON-VACUOUS MEMORY — RESPONSE dir. `--features test-gauges`.
   Gauge = the REAL crate static
   `lb_quic::h3_bridge::MAX_RETAINED_RESP_BYTES` (not a test
   stub). Multi-MiB (≥4 MiB, ≥16× ceiling) response through the
   genuine H3 upstream; assert `retained <= 262656` AND response
   body byte-identical after clean FIN. NON-VACUITY: in-worktree
   invert to `retained > ceiling`, show it FAILS with the actual
   small retained figure (proving the assertion is load-bearing
   and would fail under whole-body buffering), then revert + show
   tree clean. Evidence: mem-resp.txt, nonvacuity-inverted-probe.txt.

3. NON-VACUOUS MEMORY — REQUEST dir. Same, gauge =
   `MAX_RETAINED_BODY_BYTES`; multi-MiB request body; assert
   `retained <= 262656` AND request body byte-identical at the
   genuine upstream. Evidence: mem-req.txt.

4. BACKPRESSURE — BOTH directions, causal chain proven:
   (a) stalled client ⇒ resp_tx (depth 8) fills ⇒ M-C parks on
       `resp_tx.send().await` ⇒ stops `stream_recv` ⇒ quiche
       withholds MAX_STREAM_DATA on the upstream — assert bounded
       + body intact after resume + FIN.
   (b) stalled backend ⇒ `stream_capacity`==0 ⇒ in-hand chunk
       retained, brx not pulled ⇒ body_rx (depth 8) fills ⇒ M-A
       pump pauses the downstream client upload — assert bounded
       + correct completion after resume.
   Evidence: backpressure.txt.

5. CASE-7 SMUGGLING. Client RESET mid-request (after a partial
   body); assert the genuine upstream NEVER sees a
   truncated-as-complete request: no QUIC FIN on the request
   stream, `stream_shutdown(Write, H3_REQUEST_CANCELLED=0x010c)`,
   and the client never gets a clean 200+FIN. Evidence:
   case7.txt.

6. 7 CASES + TRAILERS-DROPPED all RUN (none #[ignore]) and the
   gauge cases are non-skippable without the flag (cond-3:
   flagless run executes strictly fewer cases / the memory
   proofs absent). Deterministic ×3 reruns of the live-path e2e.
   Evidence: cases-determinism.txt, no-gauges-run.txt.

7. INDEPENDENT COVERAGE ≥80% session code, CANONICAL script
   (NOT builder self-measure). Canonical tool per
   audit/coverage-scope.md = `cargo llvm-cov`. Session code =
   the S7 additions/edits in `lb-quic/src/h3_bridge.rs`
   (h3_to_h3_stream_resp, parse_frame_header, check_block_len,
   j2_req_event_action) + the J3 `conn_actor.rs` h3_backend
   branch. Invocation (CARGO_TARGET_DIR exported, may need
   `--no-cfg-coverage`; record the EXACT command):
     cargo llvm-cov -p lb-quic --features test-gauges \
       --no-fail-fast --summary-only [--no-cfg-coverage]
   then the per-file/region figure for h3_bridge.rs +
   conn_actor.rs ≥80% line. If `cargo-llvm-cov` is uninstallable
   offline (sandbox note in coverage-scope.md), fall back to the
   same method S6 used and record the limitation explicitly +
   escalate to lead (do NOT silently pass). Evidence:
   coverage.txt.

8. R3 NO-REGRESSION. `--features test-gauges`:
   h3_h1_resp_stream_e2e, h3_h1_stream_body_e2e (H1 leg),
   h3_h2_stream_e2e (H2 leg), lb-quic full lib+suites; plus the
   S1–S6 blast radius S6 used (lb-io, proto_translation_e2e,
   h2_proxy_e2e, h1_proxy_e2e). All green, 0 unexpected ignored;
   ignore-set unchanged vs the Phase-0/J3 baseline. Evidence:
   r3-*.txt.

9. GATES. fmt -p lb-quic --check; the CORRECTED clippy scope
   `cargo clippy -p lb-quic --all-targets --features test-gauges
   -- -D warnings` CLEAN (F-S7-1 lesson — retired --lib-only).
   Keep the no-features E0432 (MAX_RETAINED_RESP_BYTES) DISTINCT:
   it is a separate harmless artifact, canonical Phase-3 gate is
   --all-features so it resolves; do NOT conflate. Evidence:
   gates.txt.

10. FLAKE/MECHANISM (R2). Classify every failure by a mechanism
    proven from captured output; isolation-pass is NOT proof of
    "environmental". Only a proven scheduling-starvation timeout
    with zero server-side misbehaviour may be environmental.

## VERDICT RULE
H3→H3 BUILT only if 1–9 all PASS non-vacuously (esp. the §2/§3
inverted-probe load-bearing demonstration and §7 independent
≥80%). Any FAIL ⇒ proven mechanism, escalate, NOT BUILT. Write
the final VERDICT.md here + append the #7 section to
audit/h-matrix/s7-h3h3-verify.md; commit+push origin s7/verifier;
report to team-lead.

(Prepared 2026-05-19 mid-session; not yet executed — awaiting J4
acceptance.)
