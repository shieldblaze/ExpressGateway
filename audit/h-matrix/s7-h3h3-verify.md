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
