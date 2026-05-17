# SESSION 5 / task #5 P1-C RE-VERIFICATION — chunked response trailers (C4)

Author ≠ verifier (R5): builder-1 authored P1-C @ `78bdaae2`; this is
the independent verifier re-verification (author self-report NOT
trusted). Worktree: /home/ubuntu/Code/eg-verifier, detached @
`78bdaae2` (confirmed `git log --oneline -1` = "P1-C: chunked response
trailers (C4) — trailing-HEADERS-after-DATA, incremental + bounded").
User ubuntu, no sudo. Cold build (target/ was cargo-cleaned), ~36 GB
free at start / ~34 GB at end (healthy).

## The change (independently inspected via `git show 78bdaae2`)

ONLY 2 files: `h3_bridge.rs` (+273) + `h3_h1_resp_stream_e2e.rs`
(+213). **conn_actor.rs NOT modified** (`git show --stat` confirms) —
the §1.4.3 backpressure gate / bounded channel / Progressive StreamTx
are untouched (R8 / PC-3 satisfied structurally).

h3_bridge.rs: `ChunkDecoder` gains `complete` (distinct from `done`),
`trailers`, `trailer_buf`; `MAX_TRAILER_SECTION = 64 KiB`;
`take_trailers`; `parse_trailer_section` (RFC 9112 §7.1.2);
`encode_h3_trailers_frame` (deliberate ~3-line QPACK/frame-encode dup
vs the PROTO-2-12-locked `request_h3_upstream` — sound no-regression
trade-off, `request_h3_upstream` itself byte-untouched: the 3 diff
matches are all docstring/comment references in NEW code). The chunked
arm loops `while !dec.complete` (NOT `done`); EOF before `complete` ⇒
`RespAbort::ChunkedDecode`; emits ONE bounded final trailer HEADERS
`RespEvent::Bytes` after the last DATA and before `End`, only if
non-empty, under the same `total > cap` OverCap guard.

## Per-item result

| Item | Verdict | Proven mechanism / numbers |
|---|---|---|
| **C4** | **PASS** | Real-wire `r8_chunked_response_trailers_delivered_to_h3_client` (4 sub-cases — author added a 4th beyond the briefed 3) PASS 3×: (1) PC-2 coalesced trailers, (2) PC-2 split-across-reads, (3) chunked no-trailers (no spurious empty frame), (4) Content-Length carries no trailer frame. Each asserts status=200 + byte-identical 60 KB binary body + EXACT decoded trailers + clean FIN AFTER trailers. Malformed trailer section ⇒ ChunkedDecode ⇒ RESET_STREAM 0x0102, never forwarded: unit `chunk_decoder_parses_trailer_section_c4` (d) no-colon junk, (e) `:`-pseudo-header, (f) >MAX_TRAILER_SECTION each ⇒ `Err(RespAbort::ChunkedDecode)`; EOF-mid-section ⇒ ChunkedDecode proven by the chunked-arm `nr==0 while !complete` path (C3 e2e case 4). |
| **PC-1** (decoder-state regression — highest risk) | **PASS** | `r7_chunked_upstream_response_byte_identical` (split 1/7/4096/8192/1/100/99999) GREEN. ALL pre-existing C3 negatives GREEN with the new `complete`/`done` decoder: unit `chunk_decoder_rejects_malformed_framing_c3`, `chunk_decoder_decodes_split_chunks`, `chunk_decoder_tolerates_chunk_extension` PASS; e2e `c3_malformed_chunked_responses_reset_never_forward_truncated` PASS. **"junk after the zero-size terminator" STILL ⇒ ChunkedDecode**: C3 e2e case (4) `0\r\nXJUNK` — `parse_trailer_section` accumulates `XJUNK` (no `\r\n`), `feed` returns incomplete, backend EOFs, chunked arm `nr==0 while !dec.complete` ⇒ `RespAbort::ChunkedDecode` ⇒ RESET_STREAM 0x0102 (the `out.reset_code.is_some()` branch of the test is exercised); NOT accepted as a trailer. Confirmed at unit level too (case (d)). |
| **PC-2** (coalesced remainder) | **PASS** | Verified by inspection + test: `feed` sets `done` on the zero-size chunk and `continue`s the loop into the `if self.done { parse_trailer_section() }` branch IN THE SAME `feed` CALL; `parse_trailer_section` drains `self.buf` (already-buffered remainder) into the bounded `self.trailer_buf` — so trailers coalesced with `0\r\n` parse without needing later reads. r8 (1) coalesced, (2) split-across-reads, (3) bare `0\r\n\r\n` all byte-identical decoded outcome; unit (a)/(b)/(c) likewise. |
| **PC-3** (additive-only) | **PASS** | `git diff 98f4ed12 78bdaae2 -- <testfile>` is additive only: new `RespBody::ChunkedWithTrailers` variant+arm; `#[derive(Debug)]`→`#[derive(Debug, Default)]` (additive); additive `ClientOutcome.trailers` + the trailer-capture `else` branch that PRESERVES the exact `:status` parse byte-for-byte; the `#[ignore]` `r8_*_placeholder` replaced by the real `r8_*` test (authorized scaffold→real upgrade); `drive_h3_response_client_stalled` only gains `trailers: Vec::new()` in its ClientOutcome ctor (no behaviour change). NO R1–R7/C2/C3/R2/R3 logic/assertion altered/reordered/relaxed. **R2/R3 numbers byte-identical to the pre-P1-C authoritative bound**: max_retained=73,859 B; ceiling=262,656 B (=4×8×(8192+16)); body=4,194,304 B; margin 15.97×; retained/ceiling=0.2812; R3 mid-stall & peak both 73,859 B. No regression. |
| **R8** (no buffering) | **PASS** | conn_actor.rs NOT in the diff (gate/channel/Progressive untouched). The trailer frame is ONE bounded ≤MAX_TRAILER_SECTION(64 KiB) `RespEvent::Bytes` emitted after last DATA before End, under the existing `total > cap`⇒OverCap guard — body-size-independent; R2/R3 numbers unchanged confirm no buffering reintroduced. |
| **C1 / §1.3.4** | **PASS** | R5 GREEN: hard TCP RST / premature-EOF-before-CL / over-cap each ⇒ RESET_STREAM `error_code == 0x0102 == H3_INTERNAL_ERROR != 0x0100`. ClientGone intact: `r6_*` + `c2_clientgone_*` GREEN (reap_client_cancelled_responses path unaffected — conn_actor untouched). No RESET_STREAM on the trailer path (clean FIN after trailers). |
| **PROTO-2-12** | **PASS** | `h3_h1_trailers_resp_e2e` pc1 + pc2 GREEN byte-identical (request-side H3-upstream trailer path). `request_h3_upstream` byte-untouched (diff matches are docstring-only in new code; the deliberate dup avoids forking the locked path). |
| **THE AUTHORITATIVE R1 CLIPPY GATE** | **PASS** | EXACTLY `cargo clippy --all-targets --all-features -- -D warnings` (FULL workspace, NO `-p`) at `78bdaae2` → **R1_CLIPPY_EXIT=0**, 0 warnings/0 errors. Confirms the task-#9 clippy claim is accurate (no Phase-0 defect; lead's base-98f4ed12 R1_CLIPPY_EXIT=0 reproduced as clean here too). Separately `cargo clippy -p lb-quic --all-targets --all-features -- -D warnings` → **LBQUIC_CLIPPY_EXIT=0**. The `#[allow(clippy::while_let_loop)]` @ test:441 is a CORRECT false-positive scoping: (1) PRE-EXISTING — `git diff 98f4ed12 78bdaae2` shows P1-C added ONLY the comment + `#[allow]` line; the `loop`/`match write_all`/probe body are unchanged context lines (probe comment exists at 98f4ed12:380); (2) the clippy `while let Ok(())` rewrite WOULD drop the load-bearing post-match 1 ms `sock.read(&probe)` teardown probe that R6/c2_clientgone's Endless backend uses to fire `read_closed` — semantically wrong, genuine FP; (3) masks NO other lint — removing the `#[allow]` and re-running clippy fires EXACTLY ONE warning (`this loop could be written as a 'while let' loop`), nothing else; zero test behaviour/assertion/ordering change. |
| **Full workspace + determinism** | **PASS** | `cargo test --workspace --all-features`: 202 `test result: ok.` lines, ZERO failures (no FAILED/panicked/error: test failed; every result line `0 failed`). `cargo fmt --check` clean (FMT_EXIT=0, 0 diffs). h3_h1_resp_stream_e2e suite run 3× = 16 passed / 0 failed / 0 ignored each (~30.2 s, identical) — now INCLUDING the real r8 test (was 15/0/1 ignored at ba64cfdd; the placeholder is now a real passing test, nothing weakened/ignored/deleted). |

## Determinism / flake protocol

All passes deterministic 3×; R2/R3 numbers byte-identical run-to-run.
Negative controls used to harden the clippy-scoping finding (remove
`#[allow]` ⇒ exactly one FP lint; diff proves pre-existing). No
flakes; no wrong frame/code/body/order observed anywhere.

## VERDICT

**P1-C RE-VERIFIED.**

C4 chunked trailers are parsed (RFC 9112 §7.1.2), bounded (≤64 KiB),
incremental (one post-DATA HEADERS frame, conn_actor/gate untouched,
body-size-independent), and delivered byte-identical to the H3 client
with a clean FIN after trailers. Every malformed trailer/junk case ⇒
`RespAbort::ChunkedDecode` ⇒ RESET_STREAM 0x0102, never a
truncated/forwarded body as complete (PC-1 incl. junk-after-terminator
preserved). Purely additive to the test file (PC-3); R2/R3 memory &
backpressure numbers unregressed (R8); C1/§1.3.4/ClientGone intact;
PROTO-2-12 pc1/pc2 + `request_h3_upstream` untouched. The authoritative
full-workspace R1 clippy gate is EXIT 0/clean; the one `#[allow]` is a
correctly-scoped pre-existing false positive masking no other lint.
Full workspace 202/202 green, 3× deterministic, fmt clean.
