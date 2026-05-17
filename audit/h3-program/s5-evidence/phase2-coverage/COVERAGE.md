# SESSION 5 — Phase 2 isolated S5-product coverage gate

Independent measurement (verifier). Worktree
/home/ubuntu/Code/eg-verifier, detached @ `2fa417aa`
(`git log --oneline -1` = "verify(s5/task5): P1-C chunked trailers
(C4) @78bdaae2 RE-VERIFIED"). User ubuntu, no sudo.

## Tooling

- `cargo-llvm-cov 0.8.7` (already installed at `~/.cargo/bin`, NOT
  reinstalled; `rustc 1.85.1`, llvm-tools from the toolchain).
- Exact command (single pass — build instrumented + run + lcov):
  ```
  cargo llvm-cov --all-features -p lb-quic --tests \
    --lcov --output-path audit/h3-program/s5-evidence/phase2-coverage/s5.lcov \
    -- --test-threads=2
  ```
  All lb-quic test binaries ran (incl. `h3_h1_resp_stream_e2e`,
  `h3_h1_trailers_resp_e2e`, `h3_h1_stream_body_errors_e2e`, and the
  `lb_quic` lib unittests holding `chunk_decoder_*` /
  `chunk_decoder_parses_trailer_section_c4`). Zero test failures.
- Raw evidence: `s5.lcov` (4227 lines, per-line `DA:` hit counts) +
  `s5.json`. Isolation computed by intersecting the lcov `DA:` set
  with the exact S5-added/changed PRODUCT line ranges from
  `git diff bd2e6dca..2fa417aa -- crates/lb-quic/src/conn_actor.rs
  crates/lb-quic/src/h3_bridge.rs` (test files & pre-S5 lines excluded
  from BOTH numerator and denominator).

## S5 product line ranges measured (isolated)

| File | S5 region | exec lines | covered | % |
|---|---|---|---|---|
| conn_actor.rs | `reap_client_cancelled_responses` call site (282–286) | 5 | 5 | 100.0 |
| conn_actor.rs | `reap_client_cancelled_responses` fn (317–344) | 22 | 20 | 90.9 |
| h3_bridge.rs | `encode_h3_trailers_frame` (912–917) | 6 | 6 | 100.0 |
| h3_bridge.rs | `ChunkDecoder::new` C4 inits (988–990) | 3 | 3 | 100.0 |
| h3_bridge.rs | `take_trailers` (997–999) | 3 | 3 | 100.0 |
| h3_bridge.rs | `feed` C4 dispatch + `parse_trailer_section` (1008–1149) | 83 | 76 | 91.6 |
| h3_bridge.rs | chunked-arm `while !complete` + trailer emission (1382–1428) | 30 | 22 | 73.3 |
| **TOTAL (isolated S5 product)** | | **152** | **135** | **88.82** |

**ISOLATED S5 PRODUCT COVERAGE = 135/152 = 88.82% ≥ 80% → PASS.**

## Every uncovered S5 line — named + classified

### conn_actor.rs (2 lines)
- **329, 330** — the wrapped string-literal *continuation* of the
  single `tracing::debug!(...)` call at 326–331. Lines 326/327/328/
  331/332 ARE covered ⇒ the macro executes; 329–330 are a coverage
  line-attribution artifact of a multi-line string literal, no
  independent branch. **Effectively covered (tool artifact), not a
  real gap.**

### h3_bridge.rs `feed`/`parse_trailer_section` (7 lines)
- **1012** `if self.complete { return Ok(()) }` — re-feed after the
  message already completed. The producer stops feeding once
  `complete` (chunked arm loops `while !dec.complete`), so this
  defensive idempotent guard is **unreachable in the wired path**;
  retained as a safe no-op. Not a behavioural gap.
- **1027** `Some(0)` arm partial-CRLF `return Ok(())` — needs the read
  boundary to split *exactly* between a chunk body and its trailing
  CRLF with <2 bytes buffered. Reachable in principle (adversarial
  fragmentation) but not produced by the deterministic test backends;
  defensive partial-read handling, behaviourally equivalent to the
  covered `Some(remaining)` partial path. Low-value gap.
- **1041** `buf.get(..take)` `None ⇒ ChunkedDecode` — `take =
  remaining.min(buf.len())` so the slice is always valid.
  **Unreachable-by-construction** defensive guard.
- **1059** `buf.get(..nl)` `None ⇒ ChunkedDecode` — `nl` is a
  `windows(2).position` index `< buf.len()`, slice always valid.
  **Unreachable-by-construction** defensive guard.
- **1051** chunk-size line longer than `MAX_CHUNK_SIZE_LINE` with NO
  CRLF yet seen — distinct from the covered `nl > MAX_CHUNK_SIZE_LINE`
  (1055/1056, hit): this is the "huge size token, terminator not yet
  arrived" sub-case. Reachable (hostile unterminated size line);
  small smuggling-guard gap, behaviour identical to the covered
  sibling.
- **1120** `trailer_buf.len() > MAX_TRAILER_SECTION` with NO complete
  line — distinct from the covered coalesced-overflow guard at 1111
  (hit by C4 unit case (f)). This is the "oversized trailer section
  arriving WITHOUT a CRLF, split across reads" sub-case. Reachable;
  small gap, identical reject behaviour to the covered 1111/1112.
- **1144** `if name.starts_with(':') ⇒ ChunkedDecode` (RFC 9114 §4.3
  trailer pseudo-header reject) — **unreachable-by-construction.**
  `line.split_once(':')` takes the FIRST `:`, so `name` can never
  contain `:`; for a leading-colon input (`:status: 200`, C4 unit
  case (e)) `name` is `""` and is rejected one line earlier by the
  covered empty-name guard (1137–1138, hit). The pseudo-header guard
  is dead defensive code (kept for parity with the request-side
  trailer guard); the *intended* security property (a `:`-pseudo
  trailer ⇒ ChunkedDecode) IS enforced and IS tested — just via the
  empty-name path, not line 1144.

### h3_bridge.rs chunked-arm (8 lines)
- **1386, 1387** `stream.read` Err ⇒ `RespEvent::Reset` +
  `UpstreamReset`, INSIDE the chunked body loop. EOF (`nr==0`, 1390/
  1393/1394) IS covered (C3 EOF-before-terminator). A hard TCP RST
  *during a chunked-body read* is not exercised: R5's RST test uses
  Content-Length framing. **Reachable gap (small).** Proposed test:
  an `RstMidBody`-style backend with `Transfer-Encoding: chunked`
  (RST after a partial chunk) ⇒ assert RESET_STREAM 0x0102.
- **1397, 1398** `dec.feed(..) Err ⇒ propagate e + Reset`, in the
  loop body. The C3 malformed-chunked e2e cases DO drive a decode
  error, but the raw backend writes the whole malformed response in
  one segment so it is consumed via the *post-head `body_prefix`*
  `dec.feed` (the separate covered branch ~1367), not the
  subsequent-read branch at 1396–1398. **Reachable gap (small);
  behaviourally identical** to the covered body_prefix decode-error
  path. Proposed test: a chunked backend that writes a valid first
  chunk, flushes, then writes malformed framing in a *second* socket
  write (forces the error onto the loop's `dec.feed`).
- **1418, 1419** `encode_h3_trailers_frame` Err ⇒ Reset +
  `UpstreamReset` — QPACK-encoding a `Vec<(String,String)>` of decoded
  trailer fields does not fail for any input the decoder produces.
  **Unreachable-by-construction** defensive guard.
- **1424, 1425** trailer-frame `total > cap ⇒ OverCap` — the trailer
  HEADERS frame is ≤ `MAX_TRAILER_SECTION` (64 KiB) and `cap` is
  `MAX_RESPONSE_BODY_BYTES` (64 MiB); this fires only if the body
  already pushed `total` to within 64 KiB of 64 MiB. Reachable only
  by a contrived ~64 MiB chunked body + trailers; **negligible-value
  gap**, identical OverCap behaviour to the covered body-path OverCap
  (1422–1425's sibling on the DATA path, hit).

## Reachable-gap summary (none block the gate)

Of the 17 uncovered lines: **6 are unreachable-by-construction**
(1041, 1059, 1144, 1418, 1419, plus 1012 effectively-unreachable in
the wired path) and **2 are a tracing-string tool artifact** (329,
330, the macro IS executed). The remaining ~9 are
defensive/error-fragmentation siblings of COVERED paths (partial-read
CRLF split, unterminated oversize size-line / trailer section, RST or
2nd-write decode-error mid-chunked-body, trailer-over-cap). NONE is an
uncovered S5 *behavioural* path: every S5 feature path —
ClientGone reap + receiver/StreamTx teardown, trailer parse
(coalesced + split + bare + pseudo/oversize/junk reject), trailer
frame encode + emission, the `complete`-vs-`done` loop, EOF-before-
complete ⇒ ChunkedDecode — is exercised by `r6_*`, `c2_clientgone_*`,
`r8_chunked_response_trailers_delivered_to_h3_client`, `c3_*`,
`r7_*`, and the `chunk_decoder_*` / `chunk_decoder_parses_trailer_
section_c4` unit tests. The optional hardening tests proposed above
(chunked-RST, 2nd-write chunked-decode-error) would lift the figure
further but are not required: the isolated aggregate already clears
the ≥80% gate with margin.

## VERDICT

**S5 COVERAGE PASS — 88.82% ≥ 80% (isolated S5 product lines,
135/152).** Uncovered lines named & classified above: predominantly
unreachable-by-construction defensive returns + a tracing-string
tool artifact + a few low-value error-fragmentation siblings of
covered paths; zero uncovered S5 behavioural logic.
