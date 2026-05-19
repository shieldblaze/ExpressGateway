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
