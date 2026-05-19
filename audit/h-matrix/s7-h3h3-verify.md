# S7 H3→H3 Cell — Independent Per-Increment Verification (R5)

Verifier: `verifier` (author != verifier on every increment, R5).
Worktree: /home/ubuntu/Code/s7-verifier (read-only audit of builder-1's
branch; no source edited, no merge performed).
CARGO_TARGET_DIR exported to /home/ubuntu/Code/ExpressGateway/target for
all cargo runs. Disk stayed 31G free (floor 12G).

================================================================
J1 — M-C recv half + orchestrator skeleton (bodyless path)
================================================================
Date (UTC): 2026-05-18T22:?? audit run
TARGET: s7/builder-1 @ 0ce98badfeff7899c31a6943747a15f84eb6dcee
Parent: 28710c18 (e641f2b4 — the integrated P0 base — is a verified
ancestor; rebase-onto confirmed). Remote origin/s7/builder-1 == this
commit (lead-directed force-with-lease rebase verified).

VERDICT: **PASS** (all 7 scope items). J2 may proceed.

----------------------------------------------------------------
1. R3 NON-REGRESSION — PASS
----------------------------------------------------------------
`git show 0ce98bad --stat`: exactly 2 files —
  .gitignore                      | 1 +
  crates/lb-quic/src/h3_bridge.rs | 681 ++++ (681 ins, 1 del)
The single "deletion" is ONLY the `use lb_h3::{...}` import line,
replaced by a multi-line form that ADDS `DEFAULT_MAX_PAYLOAD_SIZE` and
retains every prior symbol (H3Frame, QpackDecoder, QpackEncoder,
decode_frame, encode_frame) — no semantic deletion. Everything else is
a purely additive insertion after `h3_to_h3_roundtrip`'s closing line
(@@ -2807,6 +2809,620) plus new tests after the test mod (@@ -3588,4
+4204,67). `h3_to_h3_roundtrip` body and conn_actor.rs are BYTE-
UNTOUCHED (conn_actor.rs not in the commit). The new fn
`h3_to_h3_stream_resp` has NO caller (additive only — J1 changes no
behaviour). `.gitignore` adds exactly one line:
`audit/h-matrix/s7-j1-scratch.md`. The scratch note is NOT in any git
history (`git log --all -- audit/h-matrix/s7-j1-scratch.md` empty) —
builder-1's pre-push amend confirmed.

----------------------------------------------------------------
2. MECHANISM (decode_varint header-parse + streamed DATA) — PASS
----------------------------------------------------------------
`parse_frame_header` (h3_bridge.rs:3411) is a faithful free-fn
analogue of the R8-verified M-A `StreamRxBuf::try_parse_frame_header`
(parent :500): identical `lb_h3::decode_varint` calls for the type and
length varints; identical classification — `Err(H3Error::Incomplete)`
⇒ `None` (need more), other `Err` ⇒ `Some(Err)` (malformed),
both decode ⇒ `Some(Ok((type,len)))`. The byte-at-a-time feed loop
(:3116-3135) mirrors M-A's :363 guard exactly: `hdr.push(b)` then
`hdr.len() > MAX_FRAME_HEADER_BYTES (=16)` ⇒ `RespAbort::BadHead`;
malformed-varint ⇒ `BadHead`. NO `decoded_body`, NO `.collect()` of
body, NO `Vec<u8>` body accumulation: `RecvState::InData` holds only
`remaining: usize`; payload is streamed in ≤`H3_RESP_CHUNK_MAX` slices
via `encode_h3_data_frame` onto `resp_tx` and the consumed prefix is
`rx_tail.drain(..pos)`-dropped every loop iteration. `rx_tail` bound =
a ≤16 B partial header + one drain iteration's transient bytes;
response-size INDEPENDENT.

----------------------------------------------------------------
3. G1 (DoS-rejection parity) — PASS
----------------------------------------------------------------
`check_block_len` (:3398) rejects `len > DEFAULT_MAX_PAYLOAD_SIZE`
(=1_048_576, the SAME bound the old `decode_frame` enforced) and is
called for FRAME_HEADERS (:3149) and unknown/control frames (:3163)
BEFORE any `block.extend_from_slice` buffering. DATA
(`RecvState::InData`) NEVER calls `check_block_len` and NEVER sizes a
buffer from `payload_len` (binding cond 3) — only the `remaining`
counter. Cumulative `total` is cap-checked at every emit (:3201,
:3265, :3297) ⇒ `Err(RespAbort::OverCap)` past `cap`.

----------------------------------------------------------------
4. G3 / A1 (surface) — PASS
----------------------------------------------------------------
Commit touches NO lb-h3/lb-io/quiche/codec files. FRAME_DATA,
FRAME_HEADERS, encode_h3_data_frame, encode_h3_headers_frame,
encode_h3_trailers_frame, encode_h3_response all pre-exist in parent
28710c18. lb-h3 exports used (decode_varint, MAX_VARINT,
DEFAULT_MAX_PAYLOAD_SIZE) are already `pub` in lb-h3 (varint.rs:17/27,
frame.rs:67). Only h3_bridge.rs + already-public lb-h3 exports.

----------------------------------------------------------------
5. Scaffold (Chunk-first ⇒ inline 502) — PASS
----------------------------------------------------------------
The `Some(ReqBodyEvent::Chunk(_))` ⇒ inline(502) branch (:2937-2947)
is an INTENTIONAL increment scaffold, NOT stale (S6 precedent
addressed): the doc comment (:2842-2849) names the successor —
"wholesale-replaced in J2 (the streaming request-DATA pump) before any
J3 rewire" — and proves it unreachable on the live path (the only live
H3→H3 caller until J3 is the bodyless `h3_to_h3_roundtrip`; J3 rewires
only after J2). Item 1 independently confirmed the fn has no caller. It
emits a deterministic 502 so a J1-only build cannot forward a
truncated request.

----------------------------------------------------------------
6. Independent scoped self-check on 0ce98bad — PASS
----------------------------------------------------------------
(detached @ 0ce98bad, CARGO_TARGET_DIR exported)
  cargo fmt -p lb-quic -- --check ............ CLEAN (exit 0)
  cargo test -p lb-quic --lib ................ 24 passed; 0 failed;
      0 ignored — INCLUDES `h3_bridge::tests::
      s7_j1_recv_half_frame_machinery ... ok`
  cargo clippy -p lb-quic --lib .............. CLEAN (exit 0)
The new unit test is non-vacuous: (a) parser-vs-codec (type,len)
agreement for HEADERS+DATA, (b) partial header ⇒ None, (c) multi-slice
DATA reassembly byte-identity (no accumulation/corruption), (d) the
REAL module `check_block_len` G1 boundary at exactly
DEFAULT_MAX_PAYLOAD_SIZE. Builder self-measure independently
reproduced.

----------------------------------------------------------------
7. clippy --all-targets (no --features) artifact — ADJUDICATED:
   KNOWN PRE-EXISTING, NOT a J1 regression — PASS
----------------------------------------------------------------
Mechanism: `MAX_RETAINED_RESP_BYTES` is
`#[cfg(any(test, feature = "test-gauges"))]` at h3_bridge.rs:589-590;
integration test h3_h1_resp_stream_e2e.rs:1113/:1220 imports it. Under
`clippy -p lb-quic --all-targets` WITHOUT `--features test-gauges` the
symbol is cfg'd out of the lib while the test still imports it ⇒
`E0432 unresolved import MAX_RETAINED_RESP_BYTES` (exit 101). This is a
feature-gate / scoped-invocation mismatch, not a code defect.
PROOF it is pre-existing, not J1-introduced:
  * J1 (0ce98bad) does NOT touch MAX_RETAINED_RESP_BYTES, its cfg
    gate, or h3_h1_resp_stream_e2e.rs (none in the commit).
  * Reproduced BYTE-IDENTICALLY on parent 28710c18 (pre-J1): same
    E0432, exit 101.
  * Phase-0 `--all-features` clippy on integrated e641f2b4 was CLEAN
    (verifier task #2 baseline) — the symbol resolves with the feature
    enabled.
The scoped self-check contract is `clippy -p lb-quic --lib` (CLEAN
here). The `--all-targets` no-features invocation is outside J1's
scoped self-check and is a long-standing test-harness feature-gate
artifact.

================================================================
J1 VERDICT: PASS — mechanism sound, additive-only, no regression.
J2 is clear to start.
================================================================


================================================================
J2 — M-C request/send half (streaming request-DATA pump)
================================================================
Date (UTC): 2026-05-19 audit run
TARGET: s7/builder-1 @ 8fef9e9f9d3905af9a25a0bd3ee1637c72e39c11
Parent: 0ce98bad (accepted J1). 0ce98bad..8fef9e9f is a LINEAR
fast-forward (0ce98bad is an ancestor; no force); remote
origin/s7/builder-1 == 8fef9e9f.

VERDICT: **PASS** (all 10 scope items). J3 may proceed.

1. R3 — PASS. `git show 8fef9e9f --stat` = ONLY
   crates/lb-quic/src/h3_bridge.rs (+348 -47). conn_actor.rs NOT
   present. h3_to_h3_roundtrip CODE BODY is byte-identical J1→J2
   (brace-balanced extraction, 167 lines, diff exit 0 — the only
   nearby diff lines are in the SEPARATE h3_to_h3_stream_resp `///`
   doc block). h3_to_h3_stream_resp still has NO external caller
   (additive, zero live-path behaviour change).

2. J1-STUB-GONE — PASS. grep on the 8fef9e9f blob: `J1 STUB`,
   `unreachable in J1's wired path`, `increment-named` ⇒ ZERO
   matches (all present in J1). The Chunk arm is now
   `Some(ReqBodyEvent::Chunk(b0)) => { ... req_streaming = true }`
   (real streaming path), not an inline-502 return. The fn doc was
   REWRITTEN ("# Build scope (J1 recv half + J2 send half)" / "The
   J1 Chunk-first inline(502) placeholder has been wholesale-
   replaced ... this doc reflects the post-J2 reality — no stub
   remains") — fixed, not left stale.

3. J2-G1 (NO BUSY-SPIN — critical) — PASS. The ONLY await/park in
   the loop is the single `tokio::select!` (:3488). Arms: (a)
   `tokio::time::timeout(timeout, socket_clone.recv_from(..))`,
   (b) `ev = body_rx.recv(), if want_next` where
   `want_next = matches!(req_send, ReqSend::AwaitNext)` (:3487). NO
   bare `body_rx.try_recv()` / hot-poll anywhere in the fn (grep
   clean; pre-loop peek is awaited `body_rx.recv().await`). When
   `stream_capacity==0` the in-hand chunk is retained, `body_rx` is
   NOT pulled (`Ok(_) => { /* window closed — keep in hand, no pull
   */ }` :3194) and `req_send` stays `InHand` ⇒ `want_next==false`
   ⇒ select arm (b) disabled ⇒ task PARKS on arm (a) (recv_from is
   genuinely Pending absent a packet, bounded by the quiche
   timeout) — it SLEEPS, does not spin; body_rx fills (depth 8) ⇒
   unchanged M-A pump pauses the client (request-direction
   backpressure). `biased;` polls (a) first but (a) is genuinely
   Pending absent a UDP packet, so it does not starve (b).

4. J2-G2 — PASS. `J2ReqAction::FinNoTrailers` ⇒
   `qconn_mut.stream_send(stream_id, &[], true)` — empty final
   write, fin=true = QUIC stream FIN; doc states byte-identical to
   request_h3_upstream / H3ReqStreamBody; NOT a synthetic
   zero-length H3 DATA frame. Real DATA writes only via
   encode_h3_data_frame for ≤H3_BODY_CHUNK_MAX chunks.

5. Q-J2 — PASS. `const H3_REQUEST_CANCELLED: u64 = 0x010c` (:96).
   Doc cites RFC 9114 §8.1 verbatim AND explicitly contrasts
   conn_actor.rs:73's response-leg H3_INTERNAL_ERROR (0x0102),
   stating the proxy-as-client vs proxy-as-server asymmetry is
   intentional and "must NOT be 'fixed' to a false consistency".

6. CASE-7 SMUGGLING — PASS. AbortNoFin ⇒ NO FIN +
   `stream_shutdown(Write, H3_REQUEST_CANCELLED)` + `outcome =
   Err(RespAbort::UpstreamReset)` + break ⇒ set_reusable(false)
   (:3574). Post-loop: response_complete false ⇒ RespEvent::End
   branch NOT taken; outcome.is_ok() false ⇒ falls to best-effort
   `resp_tx.send(RespEvent::Reset)` then `return outcome` (Err).
   NEVER RespEvent::End on a partial. `j2_req_event_action` maps
   `Some(Reset) | None` (mid-body reset OR producer dropped before
   End) AND encode-failure ⇒ AbortNoFin. Pre-loop first_chunk
   encode-fail path also stream_shutdown + set_reusable(false) +
   resp_tx Reset + Err. Upstream can never see a
   truncated-as-complete request.

7. R8 REQUEST — PASS. No request-body accumulation: the only
   `.collect()` in the fn is `qconn_mut.readable().collect()`
   (small Vec<u64> of stream ids, pre-existing J1 recv pattern),
   no decoded_body, no req-body Vec<u8>, no extend_from_slice of
   request bytes. `ReqSend::InHand { frame: Bytes, sent }` holds
   ≤1 in-hand encoded DATA frame; `first_chunk: Option<Bytes>` ≤1
   peeked chunk. Real bound = depth-8 body_rx
   (H3_BODY_CHANNEL_DEPTH), unchanged M-A pump; request-body-size
   INDEPENDENT. Cap is DoS-abort only, not a memory bound (comment
   :3116-3118). J2 added/modified NOTHING in
   MAX_RETAINED_BODY_BYTES / record_retained (diff empty) — zero
   new gauge wiring, reuses the pre-existing instrument.

8. G3/A1 — PASS. Commit touches ONLY h3_bridge.rs; no
   lb-io/quiche/codec/lb-h3 files. Only already-public lb-h3
   exports used.

9. TRAILERS-DROPPED documented — PASS. const-adjacent J2ReqAction
   doc, the select End arm comment ("lossless RFC-acceptable
   downgrade, NOT silent loss ... explicitly reported as a
   scoped-out item"), the rewritten fn doc, and j2_req_event_action
   doc all document it. `Some(End { trailers: _ })` discards
   trailers ⇒ FinNoTrailers. Unit test case (b) asserts
   `End { trailers: vec![("x-trailer","v")] }` ⇒ FinNoTrailers
   ("trailers are not forwarded on the H3→H3 leg") — parity, not
   silent loss.

10. SCOPED SELF-CHECK (independently re-run on 8fef9e9f,
    CARGO_TARGET_DIR exported) — PASS.
      cargo fmt -p lb-quic -- --check ........... CLEAN (exit 0)
      cargo test -p lb-quic --lib ............... 25 passed; 0
        failed; 0 ignored — INCLUDES BOTH
        `s7_j2_request_send_decision ... ok` AND J1's
        `s7_j1_recv_half_frame_machinery ... ok` (still green;
        exactly 25 as expected).
      cargo clippy -p lb-quic --lib ............. CLEAN (exit 0)
    The J2 unit test is non-vacuous: (a) Chunk⇒SendData
    byte-identical + round-trips to original, empty-chunk⇒SendData
    (never reclassified End), (b) End-with-trailers⇒FinNoTrailers,
    (c) Reset⇒AbortNoFin, (d) None⇒AbortNoFin (truncation guard).
    `clippy -p lb-quic --all-targets` (no --features) E0432
    `unresolved import MAX_RETAINED_RESP_BYTES` is the IDENTICAL
    pre-existing test-gauges feature-gate artifact adjudicated in
    J1 (same test-file imports :1113/:1220, exit 101) — NOT
    J2-introduced (J2 touches neither that symbol nor the test
    file; the symbol's decl line merely shifted 590→610 from the
    20-line const block J2 added before it). Unchanged from the J1
    adjudication.

================================================================
J2 VERDICT: PASS — mechanism sound (single-park no-spin
backpressure proven from code, FIN-terminator, case-7 no-FIN
abort, R8 no-accumulation), additive-only, h3_to_h3_roundtrip
byte-untouched, J1 stub + doc wholesale gone. J3 is clear to
start.
================================================================


================================================================
J3 — actor rewire to streaming + delete dead code (LIVE PATH)
================================================================
Date (UTC): 2026-05-19 audit run
TARGET: s7/builder-1 @ e07b6f6347010833b614733959928b4334f4889f
Parent: 8fef9e9f (accepted J2). Linear fast-forward
8fef9e9f..e07b6f63 (8fef9e9f is ancestor; NO force);
remote origin/s7/builder-1 == e07b6f63.

VERDICT: **PASS** (all 10 scope items). One NON-BLOCKING
disclosure (commit-message overclaim, see item 8 + item 9).
J4 may proceed.

1. STRUCTURE — PASS. `git show --stat` = EXACTLY 3 files:
   conn_actor.rs (+83), h3_bridge.rs (227 ±, net −188),
   lib.rs (±2). NO lb-io/quiche/codec/lb-h3. Linear FF, no force.

2. TOKEN-PARITY (independently re-derived — NOT builder's
   annotated diff) — PASS. Extracted the verified H3→H2 template
   from PARENT 8fef9e9f conn_actor.rs:809-863 and the new H3→H3
   branch from e07b6f63:884-940; `diff`ed them myself. The branch
   body is token-identical EXCEPT exactly the 3 authorized deltas:
     Δ1  `if let Some((h2pool, addr)) = h2_backend { let h2pool =
          h2pool.clone();`  →  `if let Some((qpool, addr, sni)) =
          h3_backend { let qpool = qpool.clone();` PLUS added
          `let sni = sni.clone();`  (sni destructure + clone)
     Δ2  `h3_to_h2_stream_resp(... &h2pool, ...)` →
          `h3_to_h3_stream_resp(... &sni, &qpool, ...)` (spawned
          fn + &sni arg; arg order matches the J2-verified
          h3_to_h3_stream_resp signature)
     Δ3  warn label "H3→H2 resp stream aborted" → "H3→H3 ..."
   Every other token identical: `let addr = *addr;`,
   `mpsc::channel::<ReqBodyEvent>(H3_BODY_CHANNEL_DEPTH)`,
   `mpsc::channel::<RespEvent>(H3_RESP_CHANNEL_DEPTH)`,
   `resp_rx_by_stream.insert(sid, resp_rx)`,
   `stream_response.insert(sid, StreamTx::progressive())`,
   `resp_tasks.push(tokio::spawn(...))`,
   `MAX_RESPONSE_BODY_BYTES`, the whole
   `if fin { btx.try_send(End{trailers:Vec::new()}) } else {
   body_tx_by_stream.insert + body_pending.entry(sid).or_default()
   + decode_into_pending(sid, rx_by_stream, body_tx_by_stream,
   body_pending, &[], fin) + flush_pending(sid, body_tx_by_stream,
   body_pending) }` tail, and the closing `break;`. No
   unauthorized token difference.

3. continue→break + ordering — PASS. The new H3→H3 branch ends in
   `break;` (parity with H2 :862 / H1 :950), NOT `continue;` (the
   deleted legacy buffered branch ended in `continue;`). Branch
   order preserved h2_backend → h3_backend → select_backend.
   conn_actor.rs has EXACTLY 3 hunks (`@@ -37` import token,
   `@@ -117` h3_backend field doc reword, `@@ -861` the branch
   replacement). Lines 42-808 byte-identical J2→J3 except the one
   comment-only doc reword (79-80); select_backend..EOF
   byte-identical (the only out-of-block change is positional —
   select_backend shifted 876→941 from the longer new branch +
   leading comment; its body unchanged). Builder's out-of-block
   claim VERIFIED.

4. WORKSPACE GREP ⇒ 0 + stream_resp code unchanged — PASS.
   Independent `grep -rn "h3_to_h3_roundtrip" --include="*.rs" .`
   over the whole workspace = ZERO (incl tests + comments); zero
   in *.toml too (J3-G2 cross-crate surface-safety, S6 F-S6-3).
   `fn h3_to_h3_roundtrip` count in J3 h3_bridge.rs blob = 0 (was
   1 in J2) — fully deleted. h3_to_h3_stream_resp: 670-line span
   in both J2 and J3; stripping //+/// comment lines → 499
   code-lines each, code-only diff BYTE-IDENTICAL J2→J3 (only doc
   comments reworded).

5. R3 BOUNDARY (diff-proven byte-identical J2→J3) — PASS.
   request_h3_upstream (199 lines), h3_to_h1_stream_resp (68),
   h3_to_h2_stream_resp (47), stream_h2_response (112) — all
   byte-identical. The h3_bridge.rs J2→J3 diff added ZERO
   non-comment lines (only the roundtrip-body deletion + 13
   doc-comment rewords). conn_actor H1/H2/inline-error branches +
   drain_resp_channels/drain_body_stream/decode_into_pending/
   flush_pending are in the byte-identical regions (item 3).

6. lib.rs — PASS. One-line change: `h3_to_h3_roundtrip` token
   removed from `pub use h3_bridge::{...}`; H3Request,
   H3UpstreamResponse, request_h3_upstream RETAINED.

7. LIVE-PATH REGRESSION (independently re-run on e07b6f63,
   CARGO_TARGET_DIR exported, --features test-gauges, 30G free)
   — PASS, DETERMINISTIC.
     cargo test -p lb-quic --lib ............... 25/0/0 (incl
       s7_j1 + s7_j2)
     --test round8_h3_authority_enforced ....... 3/0/0  ×3 runs
       (THE no-regression proof: the now-LIVE H3-backend actor
       path via real quiche::accept driving h3_to_h3_stream_resp,
       passing exactly as it did vs the deleted roundtrip)
     --test h3_h2_stream_e2e ................... 10/0/0 ×3 runs
       (H3→H2 unregressed by the parity edit)
     --test h3_h1_stream_body_e2e .............. 6/0/0
     --test h3_h1_resp_stream_e2e .............. 16/0/0
       (H1 branch untouched)
   All deterministic across the 3 independent reruns of the two
   critical live-path suites. NOTE: the commit message
   transposes the last two counts (claims h3_h1_stream_body_e2e
   16/16 + h3_h1_resp_stream_e2e 6/6); my independent run shows
   the reverse (6/6 and 16/16) — both pass, so this is a cosmetic
   message error, NOT a code defect.

8. CLIPPY — PASS (adjudicated). `cargo fmt -p lb-quic --check`
   clean. `clippy -p lb-quic --lib --features test-gauges` CLEAN
   (exit 0) — production code is clippy-clean. `clippy -p lb-quic
   --tests --features test-gauges` FAILS with ONE
   `clippy::indexing_slicing` "slicing may panic" at
   h3_bridge.rs:4301 — that line is
   `assert!(parse_frame_header(&hf[..1]).is_none());`, the `&hf[..1]`
   slice in the J1-era `s7_j1_recv_half_frame_machinery` test.
   ADJUDICATED PRE-EXISTING, NOT J3-introduced: J3's h3_bridge.rs
   hunks are all ≤ line 3631 and added ZERO non-comment lines (it
   never touched the test mod); reproduced BYTE-IDENTICALLY on
   parent 8fef9e9f (J2) with the same `--tests --features
   test-gauges` invocation (same "slicing may panic", same "could
   not compile (lib test)", exit 101). It is the test-code analogue
   of the J1/J2-adjudicated artifact: a pre-existing strict-lint
   (lib.rs:67 clippy::indexing_slicing) surfaced only when --tests
   compiles the lib test mod; outside J3's scoped-self-check
   contract (-p lb-quic --lib, which is clean). The --all-targets
   no-features E0432 `unresolved import MAX_RETAINED_RESP_BYTES`
   (h3_h1_resp_stream_e2e.rs:1113/:1220) is ALSO the IDENTICAL
   pre-existing test-gauges artifact from J1/J2 — unchanged, not
   J3-introduced.

9. HOUSEKEEPING — PASS (with disclosure). git log = SINGLE clean
   J3 commit e07b6f63; message is intact prose, NO bash-
   substituted backtick garbage, NO stray /tmp/scratch/.log
   artifact in the tree, NO force-push. DISCLOSURE (non-blocking):
   the commit message's self-check section overclaims —
   (a) transposes the two h3_h1 counts (item 7), and (b) states
   "clippy (... + the live-path test targets) clean" which does
   NOT hold for `--tests` (the pre-existing item-8 lint fails it).
   Neither is a J3 code defect (both pre-existing / cosmetic);
   surfaced here per honest-reporting. J3's actual contract
   (-p lb-quic --lib + the live-path test RESULTS) all pass.

10. STALE-DOC — PASS. Zero `[`h3_to_h3_roundtrip`]` rustdoc links
    and zero `h3_to_h3_roundtrip` token occurrences anywhere in
    *.rs (consistent with item 4 — no cargo-doc break). The 13
    reworded refs describe behaviour intrinsically as historical
    prose ("former buffered, body-dropping H3→H3 round-trip ...
    deleted in J3"), NOT as dangling `[`...`]` links to a missing
    symbol. S6 stale-scaffold precedent satisfied.

================================================================
J3 VERDICT: PASS — token-for-token H3→H2 clone independently
re-derived (only the 3 authorized deltas + continue→break),
roundtrip deleted workspace-wide (grep⇒0), stream_resp code +
all R3-boundary fns byte-identical, the LIVE-path regression
suite (round8_h3_authority_enforced especially) green &
deterministic ×3. Clippy/message overclaims adjudicated
NON-BLOCKING (pre-existing artifact + cosmetic). The cell is
safe to go live; J4 is clear to start.
================================================================


================================================================
F-S7-1 — corrective increment (crate-root-denied
clippy::indexing_slicing in J1 session test code)
================================================================
Date (UTC): 2026-05-19 audit run
TARGET: s7/builder-1 @ d17e51c490466ed8583bed29430376098aa05b06
Parent: e07b6f63 (accepted J3). Linear fast-forward
e07b6f63..d17e51c4, NO force; remote origin/s7/builder-1 ==
d17e51c4.

CONTEXT: F-S7-1 = J1-introduced (`&hf[..1]` in
s7_j1_recv_half_frame_machinery), latent J1→J3 because per-
increment self-checks used `-p lb-quic --lib` (did not compile
the test target). It IS a Phase-3 blocker: lib.rs:63 crate-root
`#![deny(clippy::indexing_slicing,...)]` and the
`#![cfg_attr(test, allow(...))]` block does NOT exempt
indexing_slicing in tests, so it would fail the canonical
`clippy --all-targets --all-features -- -D warnings` (clean
pre-J1 at Phase-0). Audit clippy scope is now the CORRECTED
`cargo clippy -p lb-quic --all-targets --features test-gauges
-- -D warnings` (the `--lib`-only scope is retired).

VERDICT: **PASS** (all 6 scope items). F-S7-1 accepted; J4
(#6) unblocks.

1. STRUCTURE — PASS. `git show --stat` = ONLY h3_bridge.rs
   (+7 -1), exactly the s7_j1 test site (~:4301); no
   src/non-test code, no other file. Linear FF, no force,
   remote==d17e51c4.

2. ACCEPTANCE BAR (independently run, CORRECTED scope) — PASS.
   `cargo clippy -p lb-quic --all-targets --features
   test-gauges -- -D warnings` (CARGO_TARGET_DIR exported, NOT
   --lib) = CLEAN, exit 0. Because --all-targets compiles the
   test target and -D warnings promotes every crate-root-denied
   lint to a hard error, this clean run IS the
   sweep-completeness proof — it catches any crate-root-denied
   site the hand-sweep could have missed OR that clippy never
   reached when it aborted at :4301 in the pre-fix run.

3. ASSERTION BYTE-PRESERVING — PASS. Diff replaces
   `assert!(parse_frame_header(&hf[..1]).is_none());` with
   `let one_byte = hf.get(..1).expect("encoded HEADERS frame is
   ≥1 byte"); assert!(parse_frame_header(one_byte).is_none());`.
   `hf.get(..1)` yields `Some(&hf[..1])` — the SAME 1-byte
   prefix (`hf` is a real encoded HEADERS frame, always ≥1
   byte); `.expect()` unwraps to the identical `&[u8]`;
   `.is_none()` check unchanged; no #[ignore]/skip; no
   weakening. lib.rs cfg_attr(test) allow block exempts ONLY
   unwrap_used/expect_used/panic/match_wildcard — NOT
   indexing_slicing/todo/unimplemented/unreachable/missing_docs.
   So `&hf[..1]` (indexing_slicing) is DENIED in test code while
   `.get()`+`.expect()` (expect_used) IS test-allowed: a clean
   denied→allowed trade with byte-identical semantics.

4. LIB TEST + FMT — PASS. `cargo test -p lb-quic --lib` =
   25 passed / 0 failed / 0 ignored, with BOTH
   s7_j1_recv_half_frame_machinery AND
   s7_j2_request_send_decision still `... ok` (same
   assertions; s7_j1 via the semantically-identical .get()
   form). `cargo fmt -p lb-quic --check` clean.

5. SWEEP-COMPLETENESS CROSS-CHECK — PASS. The clean corrected
   gate (item 2) proves no other crate-root-denied/non-test-
   allowed lint remains in session test code. Spot-confirmed:
   `&payload[..]` (:4355) and the `&b"..."[..]` sites
   (:3745-3748, :4258) are FULL-RANGE reborrows (RangeFull `x[..]`)
   which clippy::indexing_slicing legitimately does NOT flag
   (only fixed-index/partial-range slices can panic). Independent
   scan of both s7_j1 + s7_j2 fns for bare-index/partial-range
   slicing = NONE remaining. Builder's single-offending-site
   conclusion is sound.

6. R3 + HOUSEKEEPING — PASS. This change is
   test-assertion-binding-only, ZERO product/behavior change
   (one test assertion line + a binding line + an explanatory
   comment inside the s7_j1 test mod; no src/non-test/product
   code, no conn_actor.rs, no h3_to_h3_stream_resp). Therefore
   the J3 live-path regression suite need NOT be re-run for this
   increment — lib 25/25 + corrected-gate-clean is sufficient
   and disk-proportionate. Single clean commit d17e51c4; message
   intact prose (single-quoted heredoc — no bash-substituted
   garbage), no stray artifact, no force-push.

NOTE (kept distinct, per lead): the `--all-targets`
NO-`--features` E0432 `unresolved import MAX_RETAINED_RESP_BYTES`
is a SEPARATE, genuinely-harmless test-gauges feature-gate
artifact — the canonical Phase-3 gate is `--all-features` so the
cfg'd symbol resolves (Phase-0 proved that path clean). It is
NOT F-S7-1 and is not conflated with it.

================================================================
F-S7-1 VERDICT: PASS — corrected-scope acceptance gate
independently CLEAN (= sweep-completeness proof), assertion
byte-preserving (denied→allowed lint trade, identical
semantics), lib 25/25 both session tests green, single-site
sweep conclusion independently corroborated, zero behavior
change. F-S7-1 accepted; J4 (#6) unblocks.
================================================================


================================================================
F-S7-3 — VERIFICATION-GAP, FINALIZED (round8 never exercised the
live H3→H3 path; honest re-characterization of J1/J2/J3)
================================================================
Verifier: `verifier`. [[s2-verification-gap]]-class recurrence,
owned factually — no defensiveness. The real-wire requirement
working as designed is the point: it caught this.

INDEPENDENTLY CONFIRMED (read, cited — not inferred):
`crates/lb-quic/tests/round8_h3_authority_enforced.rs:326-327`:
    h3_backend: None,
    h2_backend: None,
`conn_actor::poll_h3`'s J3-rewired live H3→H3 branch is
`if let Some((qpool, addr, sni)) = h3_backend { … h3_to_h3_stream_resp
… }`. With `h3_backend: None` that branch is structurally NEVER
entered ⇒ `h3_to_h3_stream_resp` is NEVER called by round8. round8's
own module header confirms its purpose is the H3 authority gate; its
valid-`:authority` case dials a TcpPool probe via
`select_backend(backends)` (the H1 fallback), not the H3→H3 branch.

WHAT EACH PRIOR STEP ACTUALLY PROVED (vs not):
* J1 / J2 unit tests (`s7_j1_recv_half_frame_machinery`,
  `s7_j2_request_send_decision`): VALID but NARROW — pure
  codec/decision-table proofs, socket-less; never a real wire.
* J3 token-parity: VALID — the new conn_actor H3→H3 branch IS a
  token-for-token clone of the verified H3→H2 branch with only the 3
  authorized deltas (independently re-derived in the J3 section).
* J3 R3 no-regression for H3→H1 / H3→H2
  (`h3_h1_*`, `h3_h2_stream_e2e`): VALID — those cells have genuine
  real-wire suites that DO drive their branches.
* round8 ×3 (3/3, cited in s6 plan §4, the s7 reconfirm, the J3
  builder self-check, AND THIS DOCUMENT's J3 section
  ~lines 356-357 & 420): VALID ONLY for the H3 authority gate. The
  inference that it was the "swap no-regression proof for the live
  H3→H3 path / the now-LIVE H3-backend actor path driving
  h3_to_h3_stream_resp" was UNFOUNDED — round8 with `h3_backend:None`
  cannot and does not exercise that path. **That claim is formally
  WITHDRAWN here.**
* J4 genuine real-wire suite: the FIRST true exercise of the live
  H3→H3 path — and it caught the latent defect (F-S7-2), exactly as
  the real-wire requirement is meant to.

NET: "H3→H3 works end-to-end" was NEVER established pre-J4. J1/J2/J3
acceptance partly rested on the round8 inference, which did not hold.
The still-valid structural/unit/parity/H3→H1/H3→H2 proofs stand; the
cell is NOT BUILT pending the J5-FIX + a genuinely-green J4 suite.

ROOT OF THE AUDIT ERROR (owned): round8 was accepted as the live-path
proof from the test NAME + the s6-plan/s7-reconfirm framing, run
green ×3, WITHOUT reading its `h3_backend`/`h2_backend` wiring to
confirm it drives the rewired branch. Token/structural parity is
necessary but NOT sufficient: a verbatim-correct clone of a correct
template can still be reached only via a path no in-tree test
exercises, and the cloned-from recv-half (J1) was itself untested on
a real wire and defective (F-S7-2).

BINDING FORWARD RULE: any "swap / rewire / clone no-regression" or
"live path works" claim MUST cite a test whose BACKEND WIRING HAS
BEEN READ and confirmed to drive the specific changed branch (e.g.
`h3_backend: Some(..)` for the H3→H3 branch) — never inferred from
the test name or a plan's assertion. (Lead persists this to memory;
verifier persisted `verify-cited-test-drives-changed-path`, linked
[[s2-verification-gap]].)


================================================================
F-S7-6 STANDALONE AUDIT (#17) — s7/builder-1 @ b24d9bfd
================================================================
Verifier: `verifier` (R5 independent; builder-1 authored the fix —
verifier≠builder-1). Parent fedb5cf4 (accepted J5-FIX), linear FF
no force. Probes inserted in s7-verifier worktree only, src
restored byte-identical (sha1 61a17ef8…, git clean) — R5 honored.

VERDICT: **PASS** (all 6 items). F-S7-6 is a SOUND event-driven
idle-deadline remediation. The genuine J4 is now a deterministic
parallel-stable 7/7 — but a **VACUOUS 7/7**: case-4's pass is
proven vacuous (F-S7-4 still required). Cell remains NOT BUILT.

1. SCOPE/R-S76-4 — PASS. `git diff fedb5cf4 b24d9bfd` = ONLY
   crates/lb-quic/src/h3_bridge.rs (+84 −6); 10 hunks, each
   F-S7-6-attributable (new `H3_RESP_IDLE_TIMEOUT` const+doc incl.
   the CF-S7-RHU carry-forward note; `idle_deadline` decl before
   the send! macro; `send_progress!` macro; loop cond
   deadline→idle_deadline; the 3 progress-reset points; 3
   send!→send_progress! swaps). No stray reflow, no other file.

2. send_progress! IMPLEMENTATION (builder-flagged) — PASS.
   (a) `send_progress!` == `send!` ($tx.send($ev).await
       .map_err(|_|ClientGone)?) + idle_deadline reset. Send
       semantics unchanged.
   (b) Used at EXACTLY the 3 in-loop mid-stream egress Bytes
       sites — data_frame (:3247), trailer tf (:3311), head
       (:3343) — the complete correct set; none missed, none
       spurious. All other in-loop resp_tx egress is
       RespEvent::Reset (abort paths — correctly NOT progress).
   (c) Terminal post-loop `End` is PLAIN `send!` (NOT
       send_progress!). Post-loop disposition block BYTE-IDENTICAL
       fedb5cf4→b24d9bfd (464 lines, diff empty) — R-S76-2 PASS:
       still PrematureEof+Reset on a truncated partial, NEVER End.
   (d) Reset placement correct: response-ingress `stream_recv`
       Ok `if n>0` (:3110); request-egress `stream_send` Ok
       `if n>0` (:3050); request clean-FIN `stream_send(&[],true)`
       Ok/Done (:3455).
   (e) R-S76-5 PASS: idle_deadline reset ONLY on those 3
       application-data-progress events. NOT reset on the quiche
       timer/on_timeout, socket recv_from (transport
       keepalive/ACK), zero-byte reads/sends (explicit `if n>0`),
       or backpressure parks (select! park doesn't touch it).

3. F-S7-6 BINDING ACCEPTANCE — PASS. Genuine J4 under the
   DEFAULT PARALLEL runner, 4 reruns: **7/7 deterministic** every
   run (~10.7–11.1 s). Case 5 (8 MiB) byte-identical; **case 3
   (4 MiB) now deterministic under PARALLEL execution** — the
   F-S7-6 idle deadline fixes the case-3 contention flake proven
   in the #15 addendum. Cases 1,2,6,7 green.

4. R-S76-5 NO-HANG / R-S76-6 — PASS (by mechanism, item 2(e)).
   A dead-but-connected upstream (ACK/keepalive only, zero app
   bytes either direction) never resets idle_deadline ⇒ it fires
   within H3_RESP_IDLE_TIMEOUT (30 s) ⇒ the BYTE-UNCHANGED
   post-loop yields Err(PrematureEof)+Reset, NEVER End, NEVER an
   infinite hang. R-S76-6: a large/slow request upload with no
   response yet keeps idle_deadline alive via the request-egress
   `stream_send n>0` + request-FIN resets — not spuriously
   idle-aborted (this is exactly the case-4 flip mechanism, §5).

5. CASE-4 FAIL→PASS FLIP — ADJUDICATED (R2, proven, builder
   correctly refused to self-classify):
   * VACUITY RE-CONFIRMED at b24d9bfd: instrumented the J2 gate;
     `stream_capacity==0` fired ZERO times across the full case-4
     run. The J2 request-side backpressure gate STILL never
     fires — the harness `StallReadThenEcho` still calls
     stream_recv unconditionally so the gateway send window never
     closes. Case 4 is STILL VACUOUS; `retained ≤ ceiling` holds
     WITHOUT exercising the backpressure path.
   * FLIP MECHANISM (proven, post-loop probe): pre-F-S7-6 case-4
     = `sent_head=false`, status=None, deadline-truncated at
     5003 ms; post-F-S7-6 = `response_complete=true
     outcome_ok=true sent_head=true`. The pre-fix shared 5 s
     WALL-CLOCK `deadline` also bounded the request-send side
     (request DATA pumped in the same loop); the 4 MiB upload +
     1500 ms stall + response exceeded 5 s ⇒ the loop hit the
     wall-clock cap before the response completed ⇒ status=None.
     Post-fix the loop runs on `idle_deadline`, reset on
     request-egress progress (R-S76-6 iii), so the
     actively-progressing 4 MiB upload keeps the loop alive ⇒
     the exchange completes ⇒ case 4 "passes". This is EXACTLY
     the lead's hypothesis confirmed: the flip is F-S7-6
     removing the pre-existing 5 s truncation that was ALSO
     killing the request-heavy case-4 — NOT any new unsoundness.
   * CONCLUSION: F-S7-6 sound; the 7/7 is a VACUOUS 7/7 (case-4
     backpressure gate never fires); cell NOT BUILT; F-S7-4
     still required to make case-4 non-vacuous.

6. REGRESSION/GATES — PASS. lib 26/26 incl s7_j5; h3_h2 10/10;
   h3_h1_stream_body 6/6; h3_h1_resp_stream 16/16; round8 3/3.
   Corrected clippy -p lb-quic --all-targets --features
   test-gauges -- -D warnings CLEAN. F-S7-6 src
   (h3_bridge.rs) fmt-CLEAN; the 24 fmt diffs in
   tests/h3_h3_stream_e2e.rs are the known F-S7-5 (F-S7-4-owned,
   NOT an F-S7-6 fault). R8 memory bound unperturbed: F-S7-6 adds
   no buffering/retention (diff confirms classification/timing
   only); the #15-proven ≤~74 KB-class both-direction bounds for
   4 MiB bodies hold (cases 3/5 byte-identity passing under
   parallel corroborates).

================================================================
F-S7-6 VERDICT: PASS — event-driven idle deadline correctly
replaces the J1 5 s wall-clock truncation; R-S76-2/5/6 all met
by mechanism; scope/regression/gates clean; the case-4 fail→pass
flip is the proven benign consequence of removing the shared
wall-clock cap (NOT unsoundness). #17 accepted. The genuine J4
is a VACUOUS parallel-stable 7/7 — cell remains NOT BUILT; F-S7-4
is required to make case-4 non-vacuous, then #7.
================================================================


================================================================
# F-S7-4 INDEPENDENT AUDIT (verifier2, 2026-05-19)
================================================================
Target: s7/builder-1 @ 8d0fe450 (parent b24d9bfd = accepted
F-S7-6; grandparent fedb5cf4 = accepted J5-FIX/F-S7-2). Author
!= verifier (R5): builder-1 authored src + J4 harness; verifier2
independent, mechanism-proven (R2). Worktree: /home/ubuntu/Code/
s7-verifier (branch s7/verifier). Source-of-record baseline
sha1 (== git 8d0fe450, confirmed byte-identical):
  h3_bridge.rs        61a17ef8a0f6833614c3cf4cc4aeb8ea12ae5536
  conn_actor.rs       393e38941e32092042277b5d67f1584dd1df52c0
  lib.rs              50e832988e53389570395a837cb5b5e031904528
  h3_h3_stream_e2e.rs c1099114a8d8a9f54c013f9ebd8ab66627124870

NOTE (recovery context): prior verifier worktree was left with
UNRESTORED transient eprintln probes (FS74V markers) in
conn_actor.rs + h3_bridge.rs (detached HEAD @ 8d0fe450). These
were debug-only instrumentation, NOT source-of-record;
verifier2 discarded them (`git checkout --`) and re-established
the byte-identical target tree before any audit. Inverted-probe
discipline re-asserted.

## STEP 1 — G1 SCOPE: PASS (mechanism)
Independent re-derivation in worktree:
* `git diff --stat b24d9bfd..8d0fe450` = ONLY
  crates/lb-quic/tests/h3_h3_stream_e2e.rs | 160 +++/--- (77+/83-).
  NO src (non-test) file changed (`git diff b24d9bfd..8d0fe450
  -- crates/lb-quic/src/` EMPTY = byte-identical).
* `git diff -w b24d9bfd..8d0fe450` (non-whitespace logic delta)
  = SOLE semantic change: in `spawn_h3_upstream`, `draining` is
  now computed BEFORE the `conn.readable()`/`conn.stream_recv`
  drain, and the ENTIRE recv-drain block is gated `if draining`.
  Previously `draining` was computed AFTER stream_recv (only
  frame-decode gated). All other lines = pure rustfmt reflow
  (identical expressions re-wrapped). Other upstream modes
  (Echo / LargeResp / ResetMidResponse) hit the `_ => true`
  arm ⇒ behaviourally byte-identical.
* Ancestry: 8d0fe450^ == b24d9bfd; b24d9bfd^ == fedb5cf4 —
  linear first-parent chain fedb5cf4→b24d9bfd→8d0fe450.
* FF / no-force: s7/builder-1 reflog = every entry `commit:`
  (append-only); `git merge-base --is-ancestor b24d9bfd
  origin/s7/builder-1` TRUE; origin/s7/builder-1 ==
  8d0fe45015cfc7244e54979925c5a4b206884e37 (exact).
G1 VERDICT: PASS — scope is exactly the J4 test file; sole
non-whitespace delta is the intended F-S7-4 draining-gate;
no force-push/rebase; fast-forward only.

## STEP 2 — NON-VACUITY PROBE (BINDING): ESCALATED — gate `Ok(0)` NEVER FIRES; genuine partial-write backpressure DOES engage

Transient probes added in worktree (V2PROBE markers), src
restored byte-identical afterward (sha1 re-verified ==
source-of-record: h3_bridge.rs 61a17ef8…, conn_actor.rs
393e3894…; `git diff 8d0fe450 -- src` EMPTY). Inverted-probe
discipline satisfied.

Probe A: `eprintln` immediately BEFORE the genuine J2 gate
(`crates/lb-quic/src/h3_bridge.rs:3037` — the
`match qconn_mut.stream_capacity(stream_id)` inside
`if let ReqSend::InHand`, same stream_id, same loop iter),
logging every capacity result incl the `Ok(0)` branch.
Probe B: `eprintln` in `record_retained` logging each
new-max retained sample.

Genuine case-4 (`h3h3_e2e_request_memory_bounded_through_
stalled_backend`, `StallReadThenEcho(1500ms)`, 4 MiB POST,
`--features test-gauges`, default runner). Two independent
runs (corrected stderr capture, /tmp/v2_case4.log):

  test result  : ok (1 passed) BOTH runs — green.
  J2 gate-check calls           : 2126
  `Ok(0)` GATE-FIRED occurrences: 0   ← gate NEVER fires
  min stream_capacity observed  : 41 (never 0)
  capacity floor (persistent)   : ~111 (15×), 41 (1×), 138 (1×)
  capacity sawtooth             : drops to ~111 then quiche
                                  replenishes to ~20–68 K — a
                                  genuine flow-control cycle
  max MAX_RETAINED_BODY_BYTES   : 73728 B (≈1.76% of the 4 MiB
                                  body; 28% of ceiling 262656;
                                  body completes byte-identical)
  stream_capacity Err           : 0

MECHANISM (proven, not inferred): under the F-S7-4 stall the
upstream withholds the request-stream flow-control window, so
`stream_capacity` is driven down to a tiny floor (~41–138 B,
≪ the ≤8192 B chunk). But the code gate is
`Ok(cap_avail) if cap_avail > 0` — at capacity 111 it takes
the >0 arm and does a PARTIAL `stream_send` (~111 B accepted,
chunk stays in hand, `*sent` dribbles forward). quiche then
replenishes the window in small increments, so the value
floors just ABOVE 0 and the `Ok(0)` branch is never reached.
Backpressure is therefore GENUINE (request pump throttled to
~111-byte dribbles; retained bounded to 73728 ≪ 4 MiB ⇒ NOT
whole-body buffering), but it manifests as the partial-write
path, NOT the literal `stream_capacity==0` gate the step-2
charter names.

ADJUDICATION REQUIRED (escalated to team-lead, NOT
self-classified): step-2 charter says "CONFIRM
`stream_capacity==0` ACTUALLY FIRES … gate never fires =
FAIL regardless of green." By the literal criterion this is
a FAIL (0/2126 `Ok(0)`). By mechanism, genuine transport-
layer backpressure IS engaged and retention IS non-vacuously
bounded (the partial-write throttle is the same flow-control
phenomenon, merely surfaced as `Ok(small)`+partial-send
rather than `Ok(0)`). This is a material spec-vs-mechanism
ambiguity that the verifier must not resolve unilaterally.
F-S7-4 step-2 verdict: ESCALATED. #7 NOT entered pending
lead adjudication (F-S7-4 FAIL/ambiguous ⇒ stop, no #7).

## STEP 2 — LEAD ADJUDICATION (2026-05-19) + SUBSTANTIVE PASS

Escalation resolved by team-lead. RULING (recorded verbatim
in substance): NON-VACUITY = **PASS BY PROVEN MECHANISM**.
The literal `stream_capacity()==0` token in the original
step-2 charter was a PROXY for the real binding property, and
an over-literal one: quiche replenishes flow-control credit
incrementally, so an exact-zero floor is NOT how its flow
control manifests — the over-literal token was a charter
wording error, NOT a code defect. NOT F-S7-7: the
`Ok(cap_avail) if cap_avail > 0` partial-write path IS
correct, R8-sound QUIC flow-control backpressure (a partial
`stream_send` under a shrunk window is precisely how
backpressure should present); NO src finding opened.

SUBSTANTIVE (binding) non-vacuity criterion — POSITIVELY
PROVEN by verifier2 mechanism evidence:
 (i) THROTTLE ENGAGES: under the genuine F-S7-4 stall the
     request-stream flow-control window is driven to ≪ one
     chunk — measured 41–111 B vs the 8192 B H3_BODY_CHUNK_MAX
     (floor 111 B recurred 15×, also 41, 138; 0/2126 literal
     `Ok(0)`). The J2 pump is genuinely throttled into
     partial-writes with the chunk held in-hand. CONTRAST the
     prior VACUOUS harness (pre-F-S7-4): capacity stayed large
     and the pump never throttled (the still-vacuous 7/7 the
     #17/F-S7-6 verdict flagged).
 (ii) MEMORY BOUNDED + BODY-SIZE-INDEPENDENT: max
     MAX_RETAINED_BODY_BYTES = 73728 B = 1.76% of the 4 MiB
     request body, 28% of ceiling 262656; request body
     byte-identical at the genuine upstream. WOULD-CATCH-
     UNBOUNDED contrast: a whole-body-buffering impl would
     retain ~4 MiB (4 194 304 B) ≫ 262656 ceiling ⇒ the
     `retained <= ceiling` assertion is load-bearing and
     WOULD fail under an unbounded regression (not vacuous).

Both (i)+(ii) hold ⇒ genuine non-vacuous R8 request-side
backpressure + bounded memory. **F-S7-4 step-2: PASS** (on
the substantive criterion, lead-adjudicated — auditable, not
a silent goalpost move).

### BODY-SIZE-INDEPENDENCE CROSS-REF (R8 crux: fixed window, NOT ∝ body)
Independent prior-evidence corroboration that the ~74 KB-class
retention is INVARIANT across body size AND direction:
 * S6 H3→H2 (independent prior verifier, audit/h-matrix/
   s6-evidence/h3h2-verify/VERDICT.md:48-49,131): inverted
   probe FAILED for the expected mechanism — `retained 73859`
   for a 4 MiB body (~57× under the 262656 ceiling).
 * S7 H3→H3 case-4 (this audit): `retained 73728` for a 4 MiB
   body — SAME ~73.7–73.9 KB class, different direction.
 * S6 VERDICT also records case-5 at an 8 MiB body (≥16×
   ceiling) staying ≤ 262656 — i.e. doubling the body did NOT
   double retention.
CONCLUSION: retention is a FIXED in-flight-window class
(~74 KB), invariant across 4 MiB↔8 MiB bodies and across
H3→H2↔H3→H3 — NOT proportional to body size. Cross-ref holds
under independent check; no papering required.

### NO-BUSY-SPIN CONFIRMATION (residual-risk closure)
2126 J2 gate-checks over a ~10.9 s test (1.5 s stall window)
= ~195/s averaged, ≤~1417/s even if all concentrated in the
stall. A busy-spin would be O(1e5–1e6)/s and peg a core; the
observed rate is timer/ACK-paced. MECHANISM (code-proven,
h3_bridge.rs:3387-3413, "J2-G1: the SINGLE park point"): the
loop's sole `.await` is `tokio::select!` whose arm (a) is
`tokio::time::timeout(qconn_mut.timeout(), socket_clone
.recv_from(..))` and arm (b) `body_rx.recv()` is gated
`if want_next` — an empty `body_rx` PARKS; there is NO bare
`try_recv` hot-poll. The ~111 B-dribble path is paced by the
quiche-timeout-bounded select park, NOT busy-spinning; no
core pegged during case-4's stall (load nominal during runs).
CONFIRMED: no busy-spin.

## STEP 3 — BOUND UNDER GENUINE BACKPRESSURE: PASS

With the throttle proven engaged (step 2: capacity floored
41–111 B ≪ 8192 B chunk, partial-write dribbles), case-4's
`MAX_RETAINED_BODY_BYTES` was measured at 73728 B for a 4 MiB
(4 194 304 B) request body — well under the authoritative
ceiling 262656 B (the test itself asserts
`assert_eq!(ceiling, 262_656)` from the crate-const formula
4×(H3_BODY_CHANNEL_DEPTH=8 × (H3_BODY_CHUNK_MAX=8192 +
MAX_FRAME_HEADER_BYTES=16))). ~74 KB-class ceiling holds
(73728 ≤ 262656; 28% of ceiling, 1.76% of body). BODY-SIZE-
INDEPENDENT: corroborated by the cross-ref above (S6 H3→H2
73859 @4 MiB; S7 H3→H3 73728 @4 MiB; S6 case-5 @8 MiB still
≤262656) — fixed in-flight-window class, NOT ∝ body. The
`retained <= ceiling` + `retained > 0` assertions pass
non-vacuously across all 3 deterministic J4 reruns below.
It does NOT exceed the ceiling ⇒ NOT a new finding (no
F-S7-7). STEP 3: PASS.

## STEP 4 — GENUINE J4 7/7 + GATE SUITES: PASS

J4 genuine, NON-VACUOUS (step-2 proved case-4's gate
genuinely throttles), DEFAULT PARALLEL runner
(`cargo test -p lb-quic --features test-gauges --test
h3_h3_stream_e2e`, NO --test-threads), source-of-record
byte-identical (sha1 h3_bridge 61a17ef8, conn_actor
393e3894, test c1099114 — re-verified pre-run):
 * RUN 1: 7 passed; 0 failed; 0 ignored (11.02 s)
 * RUN 2: 7 passed; 0 failed; 0 ignored (10.94 s)
 * RUN 3: 7 passed; 0 failed; 0 ignored (10.78 s)
 DETERMINISTIC ×3. All 7 cases ran genuine (incl case-4
 gate-firing per step 2; case-7 client-RESET guard).
Gates:
 * `cargo fmt -p lb-quic -- --check` CLEAN (exit 0, no
   diff — incl tests/h3_h3_stream_e2e.rs; F-S7-5 closed).
 * `cargo clippy -p lb-quic --all-targets --features
   test-gauges -- -D warnings` CLEAN — verified GENUINE
   (forced recompile via content-cachebust, rc=0, zero
   warning/error; src restored byte-identical sha1
   61a17ef8 == baseline). Corrected scope (F-S7-1 lesson,
   --all-targets not --lib).
 * `cargo test -p lb-quic --lib` 26/26, 0 ignored.
 * `--test h3_h2_stream_e2e --features test-gauges` 10/10,
   0 ignored (H2 leg / H3→H2 cell intact — R3).
 * `--test round8_h3_authority_enforced` 3/3, 0 ignored.
 * COND-3 (flagless): `cargo test -p lb-quic --test
   h3_h3_stream_e2e` (NO test-gauges) runs exactly 4 tests
   — the 3 `#[cfg(feature="test-gauges")]` gauge cases
   (case-3/4/5 memory+backpressure) are COMPILED OUT, not
   #[ignore]'d; strictly fewer (4 < 7); the memory proofs
   genuinely require the flag and are non-vacuously present
   under it. NO `#[ignore]` anywhere in either config.
STEP 4: PASS.

F-S7-4 VERDICT: PASS — G1 scope clean (step1); non-vacuity
LEAD-ADJUDICATED PASS on the proven substantive criterion
(step2, NOT F-S7-7); bound-under-genuine-backpressure
73728 ≤ 262656 body-size-independent (step3); genuine J4
7/7 ×3 deterministic + all gate suites + cond-3 clean
(step4). F-S7-4 is the last blocker cleared ⇒ H3→H3 cell
eligible for #7 full-cell verification.

================================================================
# #7 FULL-CELL VERIFICATION (verifier2, 2026-05-19)
================================================================
Per METHOD.md. Source-of-record byte-identical throughout
(sha1 h3_bridge 61a17ef8, conn_actor 393e3894, test
c1099114 — re-verified pre each heavy run).

§1 REAL-WIRE GENUINE: PASS. J4 upstream (tests/
h3_h3_stream_e2e.rs) is a GENUINE quiche endpoint, not a
stub: `quiche::accept(&scid, None, local, from,
&mut server_cfg)` (:556) on a real `UdpSocket::bind`
loopback (:525), real `sock.recv_from` (:539) of a real
client Initial, real `conn.recv` handshake (:561); client
is real `quiche::connect` (:280) on a real socket (:273).
LIVE-PATH TRACE (satisfies "verify cited test drives changed
path"): test sets ONLY `.with_h3_backend(quic_pool, backend,
TEST_SNI)` (:233) — `h2_backend` stays None (listener.rs
:119 default). conn_actor.rs `poll_h3` (:703, called :263
with `params.h3_backend.as_ref()` :274) → the H3→H2 branch
needs h2_backend Some (None here) so falls through to
`if let Some((qpool,addr,sni)) = h3_backend` (:884) → spawns
`h3_to_h3_stream_resp(&req, addr, &sni, &qpool, brx,
resp_tx, MAX_RESPONSE_BODY_BYTES)` (:895) — the EXACT fn
holding the J2 gate (h3_bridge.rs:3037) the step-2 probe
fired inside 2126×. So the LIVE J3 h3_backend branch (the
F-S7-4-relevant changed path) IS driven, NOT a
`h3_backend:None` dead path. Binary (non-UTF-8) request +
response bodies (`binary_body`). PASS.

§2/§3 NON-VACUOUS MEMORY both dirs: PASS (covered by F-S7-4
step2/step3 + J4 cases 3/4/5 green ×3; gauge = the REAL
crate statics MAX_RETAINED_{BODY,RESP}_BYTES; retained
73728 ≪ 262656 for 4 MiB, body byte-identical; body-size-
independent cross-ref S6 73859 / S7 73728).
§4 BACKPRESSURE both dirs: PASS (req-dir = F-S7-4 step2
proven genuine throttle; resp-dir = case-5 8 MiB bounded +
byte-identical ×3).
§5 CASE-7 SMUGGLING: PASS (h3h3_e2e_client_reset_midrequest_
rsts_upstream_no_truncated_request green ×3 + ×coverage run).
§6 7 CASES + cond-3: PASS (7/7 ×3 deterministic; cond-3
flagless = 4 tests, gauge cases compiled out; 0 #[ignore]).
§8 R3 NO-REGRESSION: PASS (lib 26/26; h3_h2 10/10; round8
3/3; quic_router_leak 3/3; router_accept_path 3/3 — all
green in the instrumented coverage pass too).
§9 GATES: PASS (fmt --check clean incl test file; corrected
clippy --all-targets --features test-gauges -D warnings
genuinely clean via forced-recompile).
§10 FLAKE/MECHANISM: no failures to classify across all
deterministic reruns; no flakiness observed.

## §7 INDEPENDENT CANONICAL COVERAGE: **FAIL** (BINDING)

Canonical tool AVAILABLE locally — cargo-llvm-cov 0.8.7 +
llvm-tools-x86_64 installed (llvm-profdata present); NO
offline fallback needed. Invocation (CARGO_TARGET_DIR
exported):
  cargo llvm-cov -p lb-quic --features test-gauges
    --no-fail-fast --summary-only
J4 7/7 + all lb-quic suites ran INSTRUMENTED (h3_h3_stream
_e2e 7 passed in the cov pass — session code genuinely
exercised). Authoritative figures (lcov DA per-line, llvm-
cov's own model; whole-file cross-check 78.62% vs llvm-cov
summary 79.21% ⇒ method validated):

  WHOLE FILE h3_bridge.rs : 79.21% line (llvm-cov summary;
                            itself <80%)
  conn_actor.rs (J3 branch host): 88.42% line — PASS

  SESSION CODE (METHOD.md def — the S7 fns ONLY):
   h3_to_h3_stream_resp [2773-3540] : 67.91% line (292/430)
   j2_req_event_action  [3541-3557] : 87.50% line (7/8)
   check_block_len      [3558-3608] : 100.00% line (6/6)
   parse_frame_header   [3623-3636] : 83.33% line (10/12)
   ── SESSION TOTAL                 : 69.08% line (315/456)

69.08% ≪ the binding ≥80% METHOD.md §7 bar. Dominated by
`h3_to_h3_stream_resp` at 67.91% — 138 uncovered lines in
43 blocks, characterised by mechanism (sampled): they are
predominantly UNTESTED ERROR/ABORT + PROTOCOL-EDGE arms,
NOT dead code:
 * upstream mid-stream transport-error recovery
   (`Err(e)` → RespAbort::UpstreamReset on HEADERS/DATA/FIN
   stream_send, e.g. :2927-2932, :3058-3073, :3458-3461) —
   no J4 case injects an upstream transport fault;
 * response TRAILERS path (:3290-3312 — `:`-pseudo-header
   rejection, trailer-encode failure, trailer over-cap) —
   no J4 case sends response trailers on H3→H3;
 * `RecvState::InSkip` unknown-frame skip + check_block_len
   over-cap aborts (:3204-3217, :3349-3359) — no J4 case
   sends an unknown frame type / oversized block to skip.

These are real fault-handling and protocol-conformance
paths (R8/soundness-relevant), left unexercised by the
7-case J4 suite. Per METHOD.md VERDICT RULE ("H3→H3 BUILT
only if 1–9 all PASS … esp. the §7 independent ≥80%. Any
FAIL ⇒ proven mechanism, escalate, NOT BUILT") this is a
BINDING FAIL. Evidence: v2-7-coverage-summary.txt,
v2-7-coverage.lcov.txt.

## #7 VERDICT: **NOT BUILT** — §7 independent canonical
coverage FAIL (session code 69.08% line ≪ 80%;
h3_to_h3_stream_resp 67.91%). §1-6,8-10 all PASS; F-S7-4
itself PASS. The cell's STREAMING/BACKPRESSURE behaviour is
genuine and non-vacuous (proven), but the new session fn's
error/edge arms are under-tested vs the binding bar. NOT a
src defect (no F-S7-7) — a TEST-COVERAGE gap requiring
added J4 adversarial cases (upstream-fault, response-
trailers, unknown-frame-skip) to reach ≥80%. ESCALATED to
team-lead with mechanism; H3→H3 NOT promoted.
