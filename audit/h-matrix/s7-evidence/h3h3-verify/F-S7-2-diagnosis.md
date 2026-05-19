# F-S7-2 — INDEPENDENT PROVEN-MECHANISM DIAGNOSIS (R2/R5)

Verifier: `verifier` (independent — J1 recv-half author is builder-1,
R5). Diagnosis only; NO fix (fix = a separate gated increment).
Target under test: builder-1 worktree @ d17e51c4 (accepted J3) + the
untracked J4 test `crates/lb-quic/tests/h3_h3_stream_e2e.rs`.
Method: R2 — mechanism proven from CAPTURED runtime output + quiche
0.28.0 source, not inferred.

## SYMPTOM (reproduced)
`cargo test -p lb-quic --test h3_h3_stream_e2e --features test-gauges
h3h3_e2e_get_response_byte_identical` FAILS:
`assertion left == right failed: H3→H3 GET must return 200; left: None,
right: Some(200)`. 5/7 J4 cases fail (1-5, the ones asserting a clean
relayed 200); cases 6,7 "pass" only because they assert the ABSENCE of
a clean 200 — the broken recv-half trivially satisfies them (NOT
independent evidence of correctness).

## MEASUREMENT INSTRUMENT (reverted; src-under-test byte-identical)
Per the S6-verifier precedent ("inverted probe … then reverted; tree
clean"), I inserted measurement-only `eprintln!`s (zero logic change)
into `h3_bridge.rs` recv-half, captured, then restored the pristine
file: post-restore `sha1sum` = `cf6f60c17c65cb3ac2e19de7a77616f7f5d7a21f`
(== pre-probe), `git status --porcelain crates/lb-quic/src/h3_bridge.rs`
empty. The source-under-test is UNTOUCHED (R5).

## CAPTURED OUTPUT (case 1, bodyless GET, genuine quiche upstream)
```
FS72PROBE recv-half entered: req_streaming=false
FS72PROBE iter=1 established=true readable=[] rx_tail_len=0
FS72PROBE iter=2 established=true readable=[] rx_tail_len=0
FS72PROBE iter=3 established=true readable=[] rx_tail_len=0
FS72PROBE iter=4 established=true readable=[0] rx_tail_len=0
FS72PROBE stream_recv ERR on sid=0: InvalidStreamState(0)
            (readable said this sid was readable)
FS72PROBE post-loop: response_complete=false outcome_is_ok=false
            sent_head=false
FS72PROBE returning outcome Err = Err(UpstreamReset)
```

## ROOT CAUSE (proven, src-side — NOT a harness defect)
File/line: `crates/lb-quic/src/h3_bridge.rs`, the J1-authored recv-half
upstream-drain loop, ~lines 3034-3048 (the `stream_recv` error arm):
```
Err(quiche::Error::Done) => break,
Err(e) => { tracing::warn!(...); outcome = Err(RespAbort::UpstreamReset);
            break 'evloop; }
```
Causal chain (quiche 0.28.0, verified against its source):
1. Bodyless request ⇒ the gateway sends request HEADERS with
   `fin=true` on locally-initiated bidi stream 0
   (`headers_fin = !req_streaming`, h3_bridge.rs:2860/2865) ⇒ quiche
   `stream.send.is_complete()` becomes true.
2. The genuine `quiche::accept` upstream replies a COMPLETE, RFC-valid
   response: HEADERS + DATA + **FIN**, coalesced in one flight.
3. The recv-half `select!` `qconn.recv()` processes that flight ⇒
   `stream.recv.is_fin()` becomes true. For a locally-initiated bidi
   stream quiche `Stream::is_complete()` (quiche stream/mod.rs:818) =
   `recv.is_fin() && send.is_complete()` ⇒ **true**. quiche's
   packet-process step (quiche lib.rs:3682-3688) and the `stream_recv`
   tail (lib.rs:5744-5745) **collect (remove) the now-complete stream
   from `conn.streams`** and surface it once via `readable()`.
4. The recv-half calls `qconn.readable()` → `[0]`, then
   `qconn.stream_recv(0, ..)`. quiche `do_stream_recv` (lib.rs:5714-
   5717) does `self.streams.get_mut(0).ok_or(InvalidStreamState(0))`
   — stream 0 is already collected ⇒ returns
   `Err(quiche::Error::InvalidStreamState(0))`.
5. The recv-half maps EVERY non-`Done` `stream_recv` Err (including
   this benign "stream already complete + collected, its data was
   deliverable" state) to `RespAbort::UpstreamReset`, `break 'evloop`,
   `sent_head=false` ⇒ returns `Err(UpstreamReset)` ⇒ conn_actor
   RESET_STREAMs the front client ⇒ client sees status=None, reset.
   The valid 200 response bytes are NEVER parsed/relayed.

This is a genuine **J1 recv-half logic defect** — it does not tolerate
the upstream delivering a complete response+FIN such that quiche
completes/collects the bidi stream before the recv-half's
`stream_recv`. The genuine upstream + client in the J4 harness are
RFC-correct (upstream sends a valid complete 200; verified it is a
real `quiche::accept` endpoint, real handshake, lb_h3 framing).

## WHY VERIFICATION MISSED IT UNTIL NOW (links F-S7-3)
J1/J2 unit tests are socket-less; J3's `round8_h3_authority_enforced`
ran with `h3_backend:None` so it never drove the LIVE h3_backend
branch into `h3_to_h3_stream_resp` (F-S7-3). The first genuine
real-wire exercise is J4 — which correctly surfaced this. My J3 audit's
round8 ×3 "no-regression proof" was sound for what round8 covers but
round8 never exercised this path; that gap is F-S7-3, recorded.

## TRACTABILITY / SEVERITY (R6) — TRACTABLE, fix this session
The proven-correct sibling buffered path in the SAME file
(`request_h3_upstream`, ~h3_bridge.rs:2568) uses
`while let Ok((n,fin)) = stream_recv(..)` (benign Err just ends the
drain) and then parses whatever bytes were accumulated, completing on
`stream_finished`. The fix is a LOCALIZED recv-half change: do not
treat an Err on an already-complete/collected stream as
`UpstreamReset`; drain + parse the bytes quiche delivered and treat a
cleanly-finished stream as normal completion (mirror the proven
H3→H1/H3→H2 / `request_h3_upstream` recv discipline). Bounded, single-
function, sibling pattern exists ⇒ R6 **tractable, fixable this
session as a new plan-approval-gated increment**, owned by builder-1
(author≠verifier). The fix MUST be verified by the genuine J4
real-wire suite (5/5 of cases 1-5 passing + 6,7 still correct), never
by inference.

## SOURCE-OF-RECORD RE-CONFIRMATION (committed J4 asset e42a9b4e)
After builder-1 committed the J4 asset to s7/builder-1 @ e42a9b4e
("test(s7/J4): real-wire H3→H3 suite — 5/7 FAIL exposing F-S7-2"),
I independently re-confirmed against the source-of-record:
 * `git diff --stat d17e51c4 e42a9b4e -- crates/lb-quic/src/` = EMPTY
   — ZERO src change between my diagnosed base and the committed
   asset; `h3_to_h3_stream_resp` + the J3 conn_actor wiring are
   byte-identical to what I diagnosed. The mechanism above holds
   verbatim against the source-of-record.
 * Independent full-suite repro (detached @ e42a9b4e in s7-verifier,
   CARGO_TARGET_DIR exported, --features test-gauges): **2 passed,
   5 failed** — cases 6,7 ok (they assert ABSENCE of a clean 200, so
   the broken recv-half trivially satisfies them — NOT correctness
   evidence); cases 1-5 FAIL. Exactly matches the J4-FAIL report and
   my prior reproduction. Deterministic.
 * Cross-check vs builder-1's own s7-j4-FAIL.md: builder-1 correctly
   STOPPED at localization (recv-half; ~43 ms ≪ 5 s deadline ⇒ an
   explicit early `Err(RespAbort::*)`, not a timeout) and explicitly
   DEFERRED deep mechanism to verifier (R5). My diagnosis is
   consistent with and strictly extends builder-1's localization with
   the proven quiche mechanism.

## VERDICT
H3→H3 cell is **NOT BUILT** (R8 requires the genuine real-wire suite
to pass). F-S7-2 = real src defect, mechanism proven above and
re-confirmed against the committed source-of-record e42a9b4e. Fix is
the gated increment task #14 (J5-FIX), owned by builder-1; its
binding constraints correctly capture this mechanism (must relay the
ACTUAL 200+body — J4 case 1/2 byte-identity is the guard against a
"finish without delivering" silent regression; must NOT blanket-map
Err⇒done or case-6/7 reset/response-splitting guards regress).
#6/#7 stay blocked until the genuine J4 suite passes against the
fixed recv-half; I independently verify, never by inference.
