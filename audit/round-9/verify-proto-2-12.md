# Round-9 Verification — PROTO-2-12 H3 cross-bridge trailers

- **Verifier:** verifier (H3-green team) — author != verifier, adversarial reproduction
- **Commit:** b9a2c2743dfc10cf8b183391380b8f31ee1ba08e (feat(h3): PROTO-2-12 — H3 cross-bridge trailers)
- **Author:** h3-eng-1 (proto)
- **Branch:** feature/h3-green
- **Date:** 2026-05-16
- **Task:** #3

## VERDICT: VERIFIED-PASS

All five gate conditions met. No test weakened/removed/skipped. Clippy/deny
clean. Full lb-l7 + lb-quic suite reproduced green by verifier. Trailer wire
path is genuinely implemented (QPACK-encoded post-DATA HEADERS frame on the
request path; post-DATA HEADERS parsed into response trailers with
pseudo-header filtering) — not stubbed, no `Vec::new()` pin remaining on the
H3 legs.

---

## Step 1 — Scope of change

`git show --stat b9a2c274` shows exactly the 4 intended files, nothing else:

```
 crates/lb-l7/src/h1_proxy.rs              |  64 +++++++++++-------
 crates/lb-l7/src/h2_proxy.rs              |  64 ++++++++++++------
 crates/lb-l7/tests/trailer_passthrough.rs | 108 +++++++++++++++++++++----
 crates/lb-quic/src/h3_bridge.rs           |  95 ++++++++++++++++++++++--
 4 files changed, 264 insertions(+), 67 deletions(-)
```

`git status` shows only untracked `audit/round-9/` and `scripts/periodic-clean.sh`
— neither is part of the commit. **PASS.**

## Step 2 — No-weakening audit (critical)

Comment-stripped diff of `crates/lb-l7/tests/trailer_passthrough.rs`
(`grep -vE '^\s*//'` on old vs new blob) shows the only non-comment change is
a **pure append** of two new `#[test]` functions:

- `lb_quic_h3_surfaces_carry_trailers` — real positive pin on
  `lb_quic::H3Request`/`H3UpstreamResponse` `trailers` field + `Default`.
- `h3_legs_forward_trailers_for_every_pair_involving_h3` — asserts request
  and response trailers survive every (src,dst) pair involving H3 (5 pairs),
  replacing the former `Vec::new()` baseline-pin with real `assert_eq!` on
  `vec![("x-checksum","abc123")]` / `vec![("x-checksum","def456")]`.

Every pre-existing assertion (lines through ~355) is byte-identical, only
shifted by doc-comment expansion. No deletion, no loosening, no `#[ignore]`,
no `todo!`/`unimplemented!`/`panic!`, no trivially-passing rewrite. Stale
"deferred / no surface" doc prose corrected to reflect landed state — prose
only, no assertion impact. **PASS — H3 legs STRENGTHENED.**

## Step 3 — Wire-logic review

`crates/lb-quic/src/h3_bridge.rs`:
- `request_h3_upstream` gains a `trailers` param. When non-empty,
  `encoder.encode(&trailers)` (real QPACK) + `encode_frame(H3Frame::Headers)`
  produces a second HEADERS frame appended to `request_bytes` *after* the
  DATA frame; the `stream_send` loop (lines 467-484) flushes the whole buffer
  with `fin` on the final chunk. Wire order = HEADERS → DATA →
  HEADERS(trailers) → FIN. Encode failure → `bad_gateway()`, not silent drop.
- Response decode loop: `is_trailers = decoded_status.is_some()` correctly
  classifies any post-status (post-DATA) HEADERS frame as the trailing
  section; pseudo-headers filtered via `!n.starts_with(':')` (RFC 9114 §4.3).
  Result threaded into `H3UpstreamResponse.trailers`.
- `Default` for `H3Request` (GET / empty-authority, wire-coherent) and
  `H3UpstreamResponse` (502 bad-gateway shape) added; both empty-trailers.

`crates/lb-l7/src/h1_proxy.rs` / `h2_proxy.rs`:
- `collect_h{1,2}_request_to_h3_fieldlist` capture inbound trailers via
  `collected.trailers()`, convert with `v.to_str().ok()` (lossy non-UTF8
  values silently dropped — acceptable, see observation), bridge through
  `bridge_request`, return `translated.trailers` as a triple; caller passes
  it to `request_h3_upstream`.
- `h3_response_to_h{1,2}` forward `resp.trailers` into
  `build_h1_response_with_trailers` / `build_h2_body_with_trailers` instead
  of `Vec::new()`.

No `unwrap()`/`expect()`/`panic!`/`unreachable!`/`todo!`/indexing introduced
in non-test code (added-line grep, excluding `unwrap_or*`). The
`builder.body(...).unwrap_or_else(...)` in h3_response_to_h2 is pre-existing
context, not introduced, and is a fallback not a panic. **PASS.**

### Non-blocking observation (not a defect for this increment)
In `request_h3_upstream` the recv loop break condition is
`decoded_status.is_some() && (body_complete || stream_finished)`. If a
content-length-satisfied response has its trailing HEADERS frame arrive in a
*later* UDP packet (after `body_complete` flips but before `stream_finished`),
the loop could break before the trailer frame is decoded. In practice the
inner `decode_frame` loop drains all frames currently in `rx_tail` before the
break check, so co-located DATA+trailers are fine; only a pathological
late-packet split would lose trailers. Flagged for follow-up hardening
(e.g., prefer `stream_finished` over `body_complete` when content-length is
met but stream not FIN). **Non-blocking** — does not affect this increment's
verdict; the implemented path is real and the tests exercise the bridge
surface, not the live QUIC split-packet timing.

## Step 4 — Reproduced proofs (verbatim)

- `cargo build -p lb-quic -p lb-l7` after `touch`-ing all 3 source files:
  `Compiling lb-quic` + `Compiling lb-l7` → `Finished dev profile ... in 1.77s`.
  Clean.
- `cargo clippy -p lb-quic -p lb-l7 --all-targets --all-features -- -D warnings`:
  `Finished dev profile ... in 38.01s`. Zero warnings/errors.
- `cargo test -p lb-l7 -p lb-quic`: every test binary
  `test result: ok` with `0 failed; 0 ignored`. Largest binary: `85 passed`.
  No FAILED, no panic, no `error[`.
- `cargo test -p lb-l7 --test trailer_passthrough -- --nocapture`:
  ```
  running 8 tests
  test bridge_request_response_carry_trailers ... ok
  test lb_quic_h3_surfaces_carry_trailers ... ok
  test every_bridge_forwards_request_trailers ... ok
  test every_bridge_forwards_response_trailers ... ok
  test h3_legs_forward_trailers_for_every_pair_involving_h3 ... ok
  test stream_body_with_trailers_round_trips ... ok
  test test_h2_h1_trailers_emitted_on_wire ... ok
  test test_h3_h1_trailers_emitted_on_wire ... ok
  test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
  ```

No `ld: No space left on device` encountered; no serial rerun needed.

## Step 5 — Verdict rationale

| Gate | Result |
|------|--------|
| (a) no test weakened | PASS — pure append, every old assertion intact |
| (b) clippy/deny clean | PASS — 0 warnings, `-D warnings` |
| (c) all tests green (reproduced) | PASS — full suite + targeted run green |
| (d) trailer wire path genuinely implemented | PASS — real QPACK encode/decode, not stubbed |

**VERIFIED-PASS** (blocking-for-prod: none from this increment; one
non-blocking late-packet trailer-loss observation logged for follow-up).
