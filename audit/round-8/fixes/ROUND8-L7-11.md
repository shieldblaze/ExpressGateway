# Plan for ROUND8-L7-11 — `lb-h2::frame::decode_frame_low` PADDED flag handling (HAProxy `Properly consume padding` class)

Finding-ref:   ROUND8-L7-11 (medium, status: Open)
Files touched:
  - `crates/lb-h2/src/frame.rs`              (`decode_frame_low` lines 185–208; DATA + HEADERS arms)
  - `crates/lb-h2/tests/proptest_hpack.rs`   (extend with PADDED grammar; cross-ref L7-14)
  - new test file: `crates/lb-h2/tests/round8_padded_frame.rs`

Approach (≤500 words):

References: **HAProxy** `BUG/MEDIUM: mux-h2: Properly consume padding
for DATA frames` — HAProxy did not advance its read cursor past the
padding length; subsequent frames were parsed out of phase, creating
a smuggling primitive. **RFC 9113 §6.1** (DATA `PADDED` flag implies
1-byte `Pad Length` + N bytes of padding that are part of the
frame). Same for HEADERS (§6.2). NOTE: CONTINUATION (§6.10) does
NOT have PADDED.

Status caveat from the finding: the hot path uses hyper which handles
padding correctly. `lb-h2::frame::decode_frame_low` appears to be
**test-only** today. This is a *lesson-not-yet-paid-for* finding: the
bug is in our code waiting; we have not hit it because the decoder
is not on the live socket.

Fix:

```rust
// frame.rs — DATA arm (line 185-189)
FRAME_DATA => {
    let payload = if flags & FLAG_PADDED != 0 {
        if payload.is_empty() { return Err(H2FrameError::MalformedPadding); }
        let pad_len = payload[0] as usize;
        if pad_len + 1 > payload.len() { return Err(H2FrameError::PadLengthExceedsPayload); }
        Bytes::copy_from_slice(&payload[1 .. payload.len() - pad_len])
    } else {
        Bytes::copy_from_slice(payload)
    };
    Ok(H2Frame::Data { stream_id, payload, end_stream: flags & FLAG_END_STREAM != 0 })
}
```

Mirror the HEADERS arm with subtle ordering: when both PADDED and
PRIORITY are set, the layout is `Pad Length (1) ‖ E+Stream Dep+Weight
(5) ‖ Header Block (var) ‖ Padding (Pad Length)`. Strip the padding
*outside* first, then process the priority block inside.

CONTINUATION arm: add a code comment `// CONTINUATION has no PADDED
per RFC 9113 §6.10; ignore the bit`.

Add explicit error variants to `H2FrameError`:
- `MalformedPadding` (PADDED set but payload empty)
- `PadLengthExceedsPayload` (pad_len + 1 > payload.len())

Reference pattern: hyper's `h2` crate `frame::data::Data::load_padding`
and `frame::headers::Headers::load_padding` are the canonical Rust
reference for the exact byte layout and the error returns.

Proof:
  - `round8_padded_frame::data_with_padded_returns_inner_slice` — invariant: a DATA frame with PADDED, pad_len=1, payload `[1, b'h', b'i', b'!', 0]` returns `payload = b"hi!"` (3 bytes), not the raw 5.
  - `round8_padded_frame::data_pad_length_exceeds_payload_errors` — invariant: PADDED with pad_len > payload.len()-1 returns `Err(PadLengthExceedsPayload)`.
  - `round8_padded_frame::data_padded_empty_payload_errors` — invariant: PADDED with zero-byte payload returns `Err(MalformedPadding)`.
  - `round8_padded_frame::headers_with_padded_and_priority` — invariant: HEADERS with PADDED + PRIORITY decodes the inner header block correctly, stripping both the leading pad-length byte and the trailing N padding bytes.
  - `round8_padded_frame::continuation_padded_bit_ignored` — invariant: a CONTINUATION frame with the PADDED bit set decodes as if the bit were ignored (per RFC); we log-trace this as a peer-quirk but do not error (or we *do* error — let's discuss; HAProxy errors, hyper currently does not; recommend error to fail-closed).
  - `round8_padded_frame::data_without_padded_unaffected` — regression: a DATA frame without PADDED returns the full payload as today.

Risk / blast radius:
  - Self-contained crate (`lb-h2`). The decoder is test-only today;
    fixing the bug before live wire-in is the explicit purpose of
    this finding (lesson-not-yet-paid-for).
  - If a future refactor wires `lb-h2::decode_frame` onto a live
    socket, the fix is mandatory; today it is preventative.

Cross-ref:
  - L7-14 (proptest CVE corpus) — adds PADDED grammar to
    `tests/h2_cve_corpus.rs`.
  - The `HpackBombDetector` (`lb-h2/src/security.rs:168-219`) relies
    on the header-block byte count. Confirm that after fixing
    PADDED stripping, the ratio calculation uses the *post-strip*
    byte count, not the raw frame payload. Add an assertion in the
    detector's path.

**Audit-failure-mode theme:**
  - **Theme 4 — Multi-validator audit handoff**: CODE-2-11 shipped
    a proptest harness for `lb-h2::frame` that checks "no panic"
    but not "rejects ill-formed PADDED". The protocol validator
    never imported the RFC 9113 §6.1 PADDED grammar into the
    harness. HAProxy paid for this exact bug.
  - This finding is *also* what `ref-l7` flagged as "lesson-not-yet-
    paid-for" in the original audit-failure-mode brief: the bug is
    in our code waiting because the surface is not on the live wire
    yet.

Owner:           div-l7
Lead-approval: approved 2026-05-14 team-lead-r8
