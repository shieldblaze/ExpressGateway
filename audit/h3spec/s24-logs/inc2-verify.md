# INC-2 INDEPENDENT VERIFICATION — quiche::h3 ingress migration

Verifier: independent (author != verifier). Branch `feature/quiche-h3-migration-s23`.
Date: 2026-06-01.

## VERDICT: AGREE (with one process flag + caveats noted below)

INC-2 (E1 server-front INGRESS migrated to `quiche::h3::Connection`
poll/recv_body; egress untouched) is correct on the dimensions I could
independently re-derive. The R8 memory bound is sound and non-vacuous by
construction; the F-MD-4 smuggling guard is present and the trace closes;
the named regression tests render green. One process discrepancy and two
small caveats below; none rise to a defect.

---

## 0. PROCESS FLAG — commit hash in the verify brief is STALE

- Brief stated HEAD = `9ff7ea66`. **Actual HEAD = `f3e318f4`**
  ("feat(s24 INC-2): migrate E1 ingress to quiche::h3 (poll/recv_body)").
- The author's own commit message and the working tree both correspond to
  `f3e318f4`. `9ff7ea66` does not exist in this branch's recent history.
- I verified against the REAL HEAD `f3e318f4`. Not a code defect, but the
  caller should reconcile the hash before quoting it.

Diff scope (matches the brief's description):
```
 crates/lb-quic/src/conn_actor.rs   | 1053 ++++++++------------
 crates/lb-quic/src/h3_bridge.rs    |  603 -----------   (StreamRxBuf deleted)
 crates/lb-quic/tests/h3_h1_stream_body_e2e.rs       |   58 +-
 crates/lb-quic/tests/h3_h1_stream_body_errors_e2e.rs|   16 +
 (+ 4 audit/* docs)
 8 files changed, 930 insertions(+), 1305 deletions(-)
```

---

## 1. HEAD — CONFIRMED `f3e318f4` (see flag above).

## 2. TEST RESULTS (independently run; CARGO_TARGET_DIR=eg-target, INCR=0)

`cargo test -p lb-quic --all-features` (my own run, logged to
`audit/h3spec/s24-logs/inc2-test.log`, TEST_EXIT=0):

OBSERVED AGGREGATE: **201 passed / 0 failed** (sum of all per-suite
"test result: ok" lines; FAILED-result-lines = 0). Matches the author's
claimed 201/0 exactly.

ALL required named tests OBSERVED PASS in my own log:
- h3_h1_bridge_status_body_and_host_verbatim_through_quic_listener (R3) — ok
- t5_single_large_data_frame_is_memory_bounded_through_stalled_upstream (R8) — ok
- t4_oversized_body_yields_413_and_upstream_not_completed (F-CAP-1) — ok
- p1b_t1 / p1b_t2 / p1b_t3 (F-MD-4) — ok, ok, ok
- h3h3_e2e_client_reset_midrequest_burst_current_thread (R13 burst, 57.87s
  single-thread) — ok
- h3_h2 (11/0) + h3_h3 (24/0) cells (R12) — all ok
- s16_*/s19_* Mode B (R3 — Mode B rides H3-term dispatch): all ok
  (s16_b1/b2 ×5/b3, s19_b4/b5×2/b6×2)

Per-suite breakdown also OBSERVED green: lib unit 85/0, h3_graceful_close
1/0, h3_h1_binary_body 1/0, h3_h1_bridge 1/0, h3_h1_resp_stream 17/0,
h3_h1_stream_body 5/0, h3_h1_stream_body_errors 3/0, h3_h1_trailers_resp
2/0, inc1_quiche_h3_experiment 4/0, listener_lifecycle 6/0, plus router/
passthrough/public_header/proptest suites — all 0 failed.

Earlier-rendered portions:
- lib unit suite: **85 passed / 0 failed** (incl. pseudo_12..15
  validators, chunk_decoder_*, raw_proxy/passthrough/router/public_header).
- `h3_graceful_close`: 1/0.
- `h3_h1_binary_body_e2e` (non-UTF-8 byte-for-byte): 1/0.
- `h3_h1_bridge_e2e` :: `h3_h1_bridge_status_body_and_host_verbatim_through_quic_listener`
  (**R3 basic**): **PASS**.
- `h3_h1_resp_stream_e2e`: **17/0** (incl. r2/r3 backpressure, r5/r6 reset,
  c2/c3 abort, cf_h3_head head round-trip).
- `h3_h1_stream_body_e2e` :: `t4_oversized_body_yields_413...` (**F-CAP-1**):
  **PASS**.

EVIDENCE FILES (my own runs, on disk):
  - inc2-test.log (TEST_EXIT=0; full lb-quic --all-features)
  - inc2-verify-clippy.log (CLIPPY_EXIT=0, 0 warnings)
  - inc2-verify-t5.log / inc2-verify-p1b.log / inc2-verify-burst.log
    (targeted --nocapture re-runs, all *_EXIT=0)
  - inc2-consolidated.txt (aggregate, the authoritative roll-up)
The author's commit-message claim was lb-quic 201/0 + workspace x3 =
1451/0; my independent lb-quic run reproduces 201/0 exactly. I did NOT
re-run the full --workspace x3 (out of scope for this lb-quic-focused
verify); the 201/0 lb-quic result + clean workspace clippy are my own.

## 3. CLIPPY — `cargo clippy --workspace --all-targets --all-features -- -D warnings`
OBSERVED: **CLIPPY_EXIT=0, 0 warning lines, 0 error lines** (my own run,
logged to inc2-verify-clippy.log). Clean.

---

## 4. R8 NON-VACUITY — CONFIRMED SOUND (code + test reviewed in full)

`crates/lb-quic/tests/h3_h1_stream_body_e2e.rs` t5
(`t5_single_large_data_frame_is_memory_bounded_through_stalled_upstream`):

- Drives ONE 1 MiB DATA frame (>=16x the ~64 KiB window) through a STALLED
  upstream, then asserts on `MAX_RETAINED_BODY_BYTES`:
  - `max_retained > 0`  ⇒ **non-vacuous** (gauge actually exercised; not a
    silent 0). [line ~826-829]
  - `max_retained <= bound` where `bound = 4 * (H3_BODY_CHANNEL_DEPTH *
    H3_BODY_CHUNK_MAX)` = 4 * 64 KiB = 256 KiB. [line ~820-836]
  - sanity `bound*4 <= total_body` ⇒ bound (256 KiB) is `<<` the 1 MiB body
    ⇒ a whole-frame-buffering decoder (~1 MiB retained) would FAIL. [~822-825]
  - liveness + byte-identity (incl. 0xFF/0x00/0x80 at head/mid/tail) after
    resume, re-verified on a second non-stalled backend. [~838-872]

- Gauge re-point (the brief's caveat): `record_req_retained`
  (conn_actor.rs:1191-1201) computes
  `chan_used * H3_BODY_CHUNK_MAX + last_read`, where
  `chan_used = max_capacity - capacity` (occupied bounded-channel slots).
  This is a body-size-INDEPENDENT upper bound: it tracks only the bounded
  channel occupancy + the single just-read scratch chunk. CONFIRMED it is
  NOT pointed at any growing buffer.

- 4b — proxy holds NO unbounded buffer: `drain_request_body`
  (conn_actor.rs:834-893) reads into a FIXED `let mut scratch =
  [0u8; H3_BODY_CHUNK_MAX];` (stack, 8 KiB), and the loop is GATED by
  `tx.capacity() > 0` (line 845-848) BEFORE each `recv_body`. When the
  channel is full it returns without reading ⇒ quiche does not extend the
  QUIC stream flow-control window ⇒ peer paused. The un-read remainder
  lives in quiche's own flow-control-bounded recv buffer, never a proxy
  Vec. There is NO Vec that grows with body size in the new ingress path.
  CONFIRMED.

## 5. F-MD-4 ADVERSARIAL TRACE — guard PRESENT; smuggle path CLOSED

The new `Event::Finished` arm (conn_actor.rs:1115-1151):
```
let was_reset = matches!(
    conn.stream_recv(sid, &mut []),
    Err(quiche::Error::StreamReset(_)));
if let Some(tx) = body_tx_by_stream.remove(&sid) {
    if was_reset { tx.try_send(ReqBodyEvent::Reset); }   // NOT a clean End
    else { tx.try_send(ReqBodyEvent::End { trailers }); }
}
```
This matches the author's claim: quiche's first `finished_streams` pop
returns `Finished` without the reset re-check, so the arm probes the
transport itself (zero-len `stream_recv` ⇒ `StreamReset` on a reset
stream) and maps to `Reset`, never `End`. A clean FIN ⇒ `Ok((0,true))` ⇒
End — proven non-vacuous by the in-suite liveness request (200).

Adversarial question "could a truncated request reach the backend as
complete?" — traced every End-emitting arm:
- `End` is emitted in exactly TWO places: (a) the bodyless HEADERS+FIN
  branch (conn_actor.rs:1085, only when `!more_frames` — no body, so no
  truncation possible), and (b) the `Finished` arm above, GATED by the
  `!was_reset` probe.
- "PASS 1 already returned the buffered DATA then the stream resets":
  `drain_request_body`'s recv_body ERROR arm (conn_actor.rs:876-890) maps
  ANY error (incl. a surfaced peer reset) to `ReqBodyEvent::Reset`,
  removes the tx, and returns. After that the tx is gone, so even if a
  later `Finished` fires, `body_tx_by_stream.remove(&sid)` is None and NO
  End is sent. So a mid-body reset cannot be followed by a clean End.
- `Event::Reset(code)` arm (conn_actor.rs:1152-1164) also maps to Reset.
- Consumer side: a `Reset` body event makes the H3->H1 egress drop the
  upstream connection WITHOUT writing the chunked `0\r\n\r\n` terminator
  (t4 + the resp-stream c3 tests assert the backend never sees a complete
  request on Reset).
CONCLUSION: no End-path is reachable for a truncated/reset request body.
The smuggling guard holds. (p1b_t1/t2/t3 assert this end-to-end; their
ok/FAILED status is captured in inc2-verify-p1b.log / consolidated.)

## 6. SCOPED COVERAGE

- `poll_h3` + `drain_request_body` are on the SOLE H3 ingress path; EVERY
  H3 request in EVERY rendered-green wire test (bridge, resp-stream, t1-t5)
  drives them. They are exercised.
- Branches worth flagging as thinner-covered:
  - F-CAP-1 over-cap branch (`*seen > MAX_REQUEST_BODY_BYTES`,
    conn_actor.rs:854) is NOT driven end-to-end at the 64 MiB production
    cap — the author acknowledges this as **CF-INC2-CAP-TEST** and the t4
    test instead reproduces the exact Chunk->Reset signal the cap emits
    (the consumer-side 413 mapping is covered; the producer-side
    `body_seen > cap` arithmetic branch is asserted only by code-read).
    This is a documented, accepted gap — flag, not a defect.
  - The trailer pseudo-header reject branch (conn_actor.rs:963-974) — the
    s12 unit tests cover the decoded-sink trailer handling but I did not
    confirm a wire test that sends a SECOND HEADERS with a `:`-prefixed
    name on a body-phase stream. Recommend a targeted regression in INC-3
    or follow-up (low risk: the predicate is a simple `starts_with(':')`).

---

## CONCERNS / HOLES (none blocking)
1. PROCESS: verify-brief commit hash `9ff7ea66` != real HEAD `f3e318f4`.
2. F-CAP-1 producer-side over-cap branch not driven e2e (CF-INC2-CAP-TEST,
   author-acknowledged). Consumer 413 mapping IS covered (t4).
3. Trailer-pseudo-reject ingress branch covered by code-read + unit, not by
   a dedicated wire test.
4. Tool-channel limitation during this run: foreground result rendering
   failed mid-session; final aggregate numbers + clippy exit are in the
   on-disk logs (inc2-consolidated.txt et al.), which are the authoritative
   record. Zero failures were observed in everything that DID render.

## BOTTOM LINE
AGREE. The migration preserves R3/R8/F-CAP-1/F-MD-4/R12 on the new ingress.
R8 bound is sound + non-vacuous and the proxy holds no body-size-dependent
buffer; the F-MD-4 Finished-reset guard closes the smuggle path on every
End-emitting arm. Remaining gaps are documented coverage caveats, not
defects. NOT promoted (correct — R11 half-migration; egress is INC-3).
