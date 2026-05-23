# S9 — F-MD-4 (RST≠EOF) request-smuggling cross-cell sweep — FINAL

- Author: `protocol-eng` (S9 Phase 0a). Worktree `/home/ubuntu/Code/s9-proto`,
  branch `s9/proto-fmd4`, based on `feature/h-matrix-s9` (HEAD `8723c205`).
- Scope: the 4 BUILT cells — H3→H1, H3→H2, H3→H3, H2→H1.
- **Verdict: all 4 cells PASS by mechanism. NO defect found, NO source fix
  applied, NO new test authored** (per lead's tightly-scoped Phase B approval).
- Phase A was READ-ONLY (grep/read). Phase B ran the 4 existing real-wire
  regression tests once each, with captured evidence (below), after the lead's
  R1 ×3 determinism gate went green (1182 passed / 0 failed / 16 ignored,
  every run; fmt + clippy `--all-targets --all-features -D warnings` clean) and
  the lead independently confirmed the mechanism verdict.

## F-MD-4 restated

S8 root cause (H2): hyper maps an inbound RST_STREAM(CANCEL/NO_ERROR) to `None`
on `Body::frame()`/`poll_frame` — indistinguishable from a clean END_STREAM.
A truncated (client-cancelled) request must NEVER be relayed upstream as a
COMPLETE request. The fix pattern: gate the upstream terminator on a
POSITIVELY-CONFIRMED stream-end (`is_end_stream()` / H3 FIN), never on
`None == EOF`, and surface every abort as a body `Err` (not a clean drop).

For H3 the inbound cancel signal is a QUIC **RESET_STREAM** (peer reset its send
side) or **STOP_SENDING** (peer stopped our receive side) on the request stream,
surfaced by quiche as `Err(quiche::Error::StreamReset(code))` /
`Err(quiche::Error::StreamStopped(code))` from `stream_recv`.

---

## The shared H3 producer (single source of truth for all 3 H3 cells)

All three H3 cells consume the inbound request body through a bounded
`tokio::sync::mpsc::Receiver<ReqBodyEvent>` (`body_rx`). They do NOT touch the
QUIC stream directly. The events are PRODUCED once, in `crates/lb-quic/src/conn_actor.rs`:

- `drain_body_stream` (`conn_actor.rs:1065`) pulls bytes with
  `conn.stream_recv(sid, &mut buf)` → `Ok((n, fin))` at `:1087`. `fin` is the
  GENUINE quiche stream-FIN bit, threaded directly into `decode_into_pending`.
- `decode_into_pending` (`conn_actor.rs:1154`): appends `ReqBodyEvent::End { trailers }`
  **only** `if fin` (`:1195-1199`). `feed_body` (`h3_bridge.rs:~430-545`) returns
  only `BodyItem::Data/Trailers/TooLarge` (or `Err`) — it has NO terminator/End
  item, so a truncated body can never synthesize an `End`.
- A peer cancel surfaces from `stream_recv` as
  `Err(StreamReset(code)) | Err(StreamStopped(code))` (`conn_actor.rs:1121`) →
  `ReqBodyEvent::Reset` + full per-stream teardown (`:1128-1133`). Any other
  `stream_recv` error → `Reset` (`:1135-1143`, fail-safe). Decode error /
  `TooLarge` → `Reset` (`:1167-1192`).

**Producer invariant (verified):** `ReqBodyEvent::End` is emitted IFF a genuine
QUIC FIN was observed. A RESET_STREAM/STOP_SENDING (or any read error, decode
error, or cap breach) produces `ReqBodyEvent::Reset`, never `End`. The
`None`/closed-channel case (producer task dropped) is handled by each consumer
as an abort, never a clean end. This is the H3 analogue of the H2
`is_end_stream()` positive gate, located at the producer rather than the
consumer.

---

## Per-cell verdicts

### H2→H1 — `crates/lb-l7/src/h2_proxy.rs` — PASS (re-confirmed)
- Body consume: the request-pump `frame()`/`poll_frame` path in `proxy_request`.
  `is_end_stream()` appears 5× (count verified): `:1428`, `:1662`, `:1731`
  (and the doc-cited :1722/:1723). The upstream terminator is gated on
  `body.is_end_stream()` (positive END_STREAM), NOT on `None`.
- Abort surfacing: a constructible `PumpAbort` (`:108`) is sent as a body `Err`
  on every abort arm (`tx.send(Err(PumpAbort))` at `:1742`, `:1766`, `:1782`,
  `:1806`) — never a clean-EOF drop. Err-before-close ordering preserved.
- Mechanism unchanged from the S8 BUILT tip; the 5× `is_end_stream` gate +
  `PumpAbort` arms are present. **PASS** (will re-run the existing S8 smuggle
  regression in Phase B as the re-confirm, no new fix expected).

### H3→H1 — egress in `crates/lb-quic/src/h3_bridge.rs` `write_h1_request` — PASS
- The `lb-l7/src/h3_to_h1.rs` "bridge" is a PURE header transform
  (`body: req.body.clone()` at `:63`); it does NOT consume the body. The real
  body egress is `write_h1_request` (`h3_bridge.rs:1877`).
- Body consume: `body_rx.recv()` peek (`:1884`) + the `while let Some(ev) = body_rx.recv()`
  loop (`:1931`).
- Discrimination: `ReqBodyEvent::End` → `clean_end = true; break` (`:1940-1967`);
  ONLY then is the chunked terminator `0\r\n\r\n` written (`:2003-2008`).
  `ReqBodyEvent::Reset` → `return Ok(Aborted(413,..))` BEFORE the terminator
  (`:1969-1992`). Channel closed without End → `clean_end == false` →
  `return Ok(Aborted(502,..))`, no terminator (`:1996-2002`). The caller marks
  the pooled TCP conn non-reusable on every non-`Complete` outcome.
- A mid-body RESET_STREAM/STOP_SENDING surfaces (via the producer) as
  `ReqBodyEvent::Reset` → terminator never written → upstream never sees a
  completable request. **PASS**.

### H3→H2 — `crates/lb-quic/src/h3_bridge.rs` `H3ReqStreamBody` / `request_h2_upstream` — PASS
- Body consume: the custom `H3ReqStreamBody::poll_frame` (`:2231`) reading
  `body_rx.poll_recv` (`:2246`), via `h2_request_body_from_rx` (`:2309`) and
  `request_h2_upstream` (`:2399-2401`).
- Discrimination: `ReqBodyEvent::End` → `done = true; Poll::Ready(None)` (clean
  EOS, `:2251-2257`). `ReqBodyEvent::Reset` **OR** `Poll::Ready(None)`
  (closed channel) → `Poll::Ready(Some(Err(Box::new(H3ReqAbort))))` (`:2258-2268`).
  A body `Err` makes hyper **RST_STREAM** the H2 upstream — a truncated request
  is never presented as complete (H2 multiplexing ⇒ per-stream RST does not
  poison the connection). This is the EXACT H2→H1 Err-not-EOF pattern. **PASS**.

### H3→H3 — `crates/lb-quic/src/h3_bridge.rs` `request_h3_upstream` / `j2_req_event_action` — PASS
- Body consume: the single-park-point `ev = body_rx.recv()` arm (`:3429`),
  classified by `j2_req_event_action` (`:3535+`).
- Discrimination: `ReqBodyEvent::End` → `J2ReqAction::FinNoTrailers` →
  `stream_send(stream_id, &[], true)` (genuine QUIC FIN, `:3437-3464`).
  `ReqBodyEvent::Reset` OR closed channel (`None`) → `J2ReqAction::AbortNoFin`
  (`:3548`) → `stream_shutdown(stream_id, Shutdown::Write, H3_REQUEST_CANCELLED)`,
  NO FIN, `outcome = Err(UpstreamReset)` (`:3466-3484`). The response-completion
  is keyed off the POSITIVE `upstream_fin` gate (never `None`/EOF). The pooled
  upstream conn is set non-reusable on every exit (`:3494`). **PASS**.

---

## Verdict summary

| Cell  | Verdict | Body-consume site | Positive-end gate | Cancel → abort path |
|-------|---------|-------------------|-------------------|---------------------|
| H2→H1 | PASS    | `h2_proxy.rs` pump frame()/poll_frame | `is_end_stream()` ×5 (:1428/:1662/:1731) | `Err(PumpAbort)` (:1742/1766/1782/1806) |
| H3→H1 | PASS    | `write_h1_request` recv loop (`h3_bridge.rs:1884`,`:1931`) | `End`→`clean_end` then terminator (:1940/2003) | `Reset`→`Aborted` before terminator (:1969) |
| H3→H2 | PASS    | `H3ReqStreamBody::poll_frame` (`h3_bridge.rs:2231`/`:2246`) | `End`→`Ready(None)` (:2251) | `Reset`/`None`→`Err(H3ReqAbort)` (:2258) |
| H3→H3 | PASS    | `body_rx.recv()` park arm (`h3_bridge.rs:3429`) | `End`→`FinNoTrailers`/QUIC FIN (:3437) | `Reset`/`None`→`AbortNoFin`/no-FIN (:3466) |

Common root: the H3 producer (`conn_actor.rs:1087/1121/1195`) emits `End` IFF a
genuine QUIC FIN, and `Reset` on RESET_STREAM/STOP_SENDING/error/cap. No cell
treats `None`/EOF as a clean end.

**No AT-RISK cell found in Phase A.** All 4 already implement the F-MD-4
positive-end-gate / abort-as-error pattern by mechanism.

---

## Regression-test plan (real-wire; genuine client → real listener → router → real backend)

GOOD NEWS: each cell ALREADY ships a real-wire client-RESET-mid-body regression
test that is the exact S8 `complete=0` analogue. Phase B is therefore primarily
RE-RUN + capture, not net-new authoring (unless lead wants a hardening add).

| Cell | Test name | Location | Cancel signal injected | Load-bearing assertion (`complete=0` analogue) |
|------|-----------|----------|------------------------|-----------------------------------------------|
| H3→H1 | `p1b_t1_client_cancels_mid_body_upstream_not_completed_and_no_leak` | `crates/lb-quic/tests/h3_h1_stream_body_errors_e2e.rs:419` | real quiche client HEADERS+partial DATA (no fin) then `stream_shutdown(Write, 0x10)` (QUIC RESET_STREAM) | `saw_first_conn==1` (egress began ⇒ non-vacuous) AND `saw_terminator==0` (no chunked `0\r\n\r\n` ever to the real TCP backend) + a 2nd request through the SAME listener still succeeds (no state leak) |
| H3→H2 | `h2_e2e_client_reset_midrequest_rsts_h2_upstream_no_truncated_request` | `crates/lb-quic/tests/h3_h2_stream_e2e.rs` | client sends 2 MiB POST, `stream_shutdown(Write, 0x10c)` after ~256 KiB | `!backend_saw_complete` — the real hyper-H2 echo backend's `complete` flag (set only on a cleanly-ended request body) is FALSE; and `!(status==200 && fin)` to the client |
| H3→H3 | `h3h3_e2e_client_reset_midrequest_rsts_upstream_no_truncated_request` | `crates/lb-quic/tests/h3_h3_stream_e2e.rs` | client 2 MiB POST, `stream_shutdown(Write, 0x10c)` after ~256 KiB | `!backend_saw_complete` — the real quiche-H3 upstream stores `complete = req_fin` (genuine upstream-observed QUIC FIN) ⇒ FALSE; and `!(status==200 && fin)` |
| H2→H1 | `smuggling_rst_mid_body_never_complete_at_upstream` | `crates/lb-l7/.../h2h1_md_streaming_verify.rs` (S8 verifier suite) + `cov_within_window_rst_mid_body` (within-window) | real h2 client RSTs mid-body (post-dial >window) / within-window RST | `SMUGGLING dials=1 complete_requests=0` (post-dial abort reached, upstream sees no complete request) |

Non-vacuity of each H3 test is already designed-in: H3→H1 asserts the request
REACHED the backend (`saw_first_conn==1`) so the no-terminator assertion is
meaningful; H3→H2/H3→H3 send 2 MiB and reset after 256 KiB so egress demonstrably
began before the cancel. None are `#[ignore]`'d (confirmed by grep).

## Phase B captured evidence (per cell)

`export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target` then each test run once,
`--features test-gauges --test-threads=1 --nocapture`. All GREEN.

### H2→H1 — `SMUGGLING dials=1 complete_requests=0`
```
$ cargo test -p lb-integration-tests --features test-gauges \
    --test h2h1_md_streaming_verify smuggling_rst_mid_body_never_complete_at_upstream -- --nocapture
test smuggling_rst_mid_body_never_complete_at_upstream ... SMUGGLING dials=1 complete_requests=0
ok
test result: ok. 1 passed; 0 failed; 0 ignored; 13 filtered out; finished in 4.51s
```
`dials=1` (post-dial abort path REACHED — non-vacuous) AND `complete_requests=0`
(the H1 upstream NEVER saw the RST'd-mid-body request as a complete request).
Within-window companion:
```
$ cargo test -p lb-integration-tests --features test-gauges \
    --test h2h1_md_coverage_driver cov_within_window_rst_mid_body -- --nocapture
test cov_within_window_rst_mid_body ... COV within_window_rst_mid_body done
ok
test result: ok. 1 passed; 0 failed; 10 filtered out; finished in 2.21s
```

### H3→H1 — `saw_first_conn=1`, `saw_terminator=0` (asserted)
```
$ cargo test -p lb-quic --features test-gauges \
    --test h3_h1_stream_body_errors_e2e p1b_t1_client_cancels_mid_body_upstream_not_completed_and_no_leak -- --nocapture
test p1b_t1_client_cancels_mid_body_upstream_not_completed_and_no_leak ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 2 filtered out; finished in 1.81s
```
The test asserts on atomics (it does not print): the PASS proves
`saw_first_conn.load()==1` (`h3_h1_stream_body_errors_e2e.rs:539-544` — egress
demonstrably began, so the assertion is non-vacuous) AND
`saw_terminator.load()==0` (`:545-550` — the real TCP backend NEVER received the
chunked `0\r\n\r\n` terminator). The follow-on liveness assert (`status2 ==
UPSTREAM_STATUS`, `:556`) proves no per-stream map leak / actor-state corruption.
A non-zero terminator or a missing first-conn would panic the assert → the
GREEN result IS the `complete=0` capture.

### H3→H2 — `!backend_saw_complete` (asserted)
```
$ cargo test -p lb-quic --features test-gauges \
    --test h3_h2_stream_e2e h2_e2e_client_reset_midrequest_rsts_h2_upstream_no_truncated_request -- --nocapture
test h2_e2e_client_reset_midrequest_rsts_h2_upstream_no_truncated_request ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 9 filtered out; finished in 0.41s
```
2 MiB POST, real quiche client `stream_shutdown(Write, 0x10c)` (RESET_STREAM)
after ~256 KiB (egress began — non-vacuous). PASS proves `!backend_saw_complete`
(`h3_h2_stream_e2e.rs` — the real hyper-H2 echo backend's `complete` flag, set
only on a cleanly-ended request body, is FALSE) AND `!(status==200 && fin)` to
the client.

### H3→H3 — `!backend_saw_complete` (asserted)
```
$ cargo test -p lb-quic --features test-gauges \
    --test h3_h3_stream_e2e h3h3_e2e_client_reset_midrequest_rsts_upstream_no_truncated_request -- --nocapture
test h3h3_e2e_client_reset_midrequest_rsts_upstream_no_truncated_request ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 19 filtered out; finished in 0.46s
```
Same shape against a REAL quiche-H3 upstream whose `complete` flag is stored as
`req_fin` (`h3_h3_stream_e2e.rs:850` — the genuine upstream-observed QUIC FIN).
PASS proves `!backend_saw_complete` (no truncated-as-complete request upstream)
AND `!(status==200 && fin)`.

---

## Honest note: `StreamStopped` shares the fail-safe arm (no separate test)

Per lead's ruling, NO STOP_SENDING-variant test was added. Rationale recorded
here for the next session:

- The H3 cancel signal the request-body recv path actually observes is a peer
  **RESET_STREAM** (peer reset its *send* side), surfaced from `stream_recv` as
  `Err(quiche::Error::StreamReset(code))`. STOP_SENDING is fundamentally a
  *send-side* signal (the peer asking us to stop sending on a stream we send on)
  — on the request-body *recv* path it does not reliably surface as
  `StreamStopped` from `stream_recv`.
- `conn_actor.rs:1121` matches `Err(StreamReset(code)) | Err(StreamStopped(code))`
  in ONE arm → `ReqBodyEvent::Reset` + teardown, and `conn_actor.rs:1135` is a
  CATCH-ALL `Err(e)` that ALSO routes to `ReqBodyEvent::Reset` + teardown. So
  the body-recv error handling is **total fail-safe**: every non-`Done`,
  non-`Ok` outcome of `stream_recv` (StreamReset, StreamStopped, or anything
  else) produces `Reset`, never a synthesized `End`. A forced recv-side
  STOP_SENDING test would risk being unreachable/vacuous (the exact anti-pattern
  we reject), and even if it were reachable it would land on the SAME total
  fail-safe behaviour already locked by the RESET_STREAM tests. The mechanism is
  covered; a dedicated test is not warranted.

---

## Final per-cell table

| Cell  | Verdict | Existing test serving as the regression-lock | Captured evidence |
|-------|---------|----------------------------------------------|-------------------|
| H2→H1 | PASS    | `smuggling_rst_mid_body_never_complete_at_upstream` + `cov_within_window_rst_mid_body` | `SMUGGLING dials=1 complete_requests=0` |
| H3→H1 | PASS    | `p1b_t1_client_cancels_mid_body_upstream_not_completed_and_no_leak` | GREEN ⇒ `saw_first_conn==1` ∧ `saw_terminator==0` (asserted) |
| H3→H2 | PASS    | `h2_e2e_client_reset_midrequest_rsts_h2_upstream_no_truncated_request` | GREEN ⇒ `!backend_saw_complete` (real H2 backend) |
| H3→H3 | PASS    | `h3h3_e2e_client_reset_midrequest_rsts_upstream_no_truncated_request` | GREEN ⇒ `!backend_saw_complete` (real H3 upstream, `complete=req_fin`) |

All 4 tests are real-wire (genuine client → real listener → router → real
backend), non-vacuous, and not `#[ignore]`'d — already inside the lead's R1 ×3
determinism gate, satisfying the per-cell regression-lock requirement.

---

## Disk / environment
- `df -h /home/ubuntu`: 28 GB free at Phase A start; 27 GB free after the Phase B
  test builds (>25 GB floor — OK). Did NOT `cargo clean`.
- Phase A made ZERO cargo invocations (lead's R1 ×3 determinism gate not
  perturbed). Phase B reused the shared `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target`.
