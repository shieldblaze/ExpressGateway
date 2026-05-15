### ROUND8-L7-11 — `lb-h2::frame::decode_frame_low` ignores DATA / HEADERS padding (HAProxy `BUG/MEDIUM: mux-h2: Properly consume padding for DATA frames` class)

Reference: `audit/round-8/research/haproxy.md` lesson 12 (HAProxy did not advance its read cursor past the padding length; subsequent frames were parsed out of phase — smuggling primitive). RFC 9113 §6.1 (DATA `Padded` flag implies 1-byte `Pad Length` + N bytes of padding that are part of the frame).
Our equivalent: `crates/lb-h2/src/frame.rs:185-208` (`decode_frame_low` — `FRAME_DATA` and `FRAME_HEADERS` arms)

Severity: medium
Status:   Verified-Fixed(verifier=verify, accf6429)   <!-- INDEPENDENT VERIFICATION (verifier=verify, author=div-l7): strip_padding on DATA+HEADERS arms; HEADERS+PRIORITY ordering correct (pad outside then priority inside, length-check on unpadded slice); CONTINUATION untouched. round8_padded_frame 6/6 PASS. Bypass: pad_len+1>len rejects under-length (no int overflow), empty inner RFC-legal, empty payload+PADDED Err; no off-by-one. Decoder is test-only (finding-documented lesson-not-yet-paid-for) but now correct. See audit/round-8/verify/l7.md. -->

Divergence:
- **Reference (RFC 9113 §6.1)**: when the `PADDED` flag is set on a DATA frame, the payload is `Pad Length (1 byte) ‖ Data (variable) ‖ Padding (Pad Length bytes)`. The actual application data is the middle slice. Same shape for HEADERS with PADDED.
- **Us**: `decode_frame_low` checks `FLAG_END_STREAM` and `FLAG_PRIORITY_FLAG` but **does not check `FLAG_PADDED` (0x08)**. The entire payload (including the pad-length byte and the trailing padding bytes) is returned as `H2Frame::Data { payload: ... }` / `H2Frame::Headers { header_block: ... }`. A peer that sends `PADDED` flagged frames will be misinterpreted: the application data slice is wrong by one byte at the front (the pad-length byte) and N bytes at the back (the padding).

```rust
// frame.rs:185-189 — DATA arm
FRAME_DATA => Ok(H2Frame::Data {
    stream_id,
    payload: Bytes::copy_from_slice(payload),   // ← whole payload including pad len + padding
    end_stream: flags & FLAG_END_STREAM != 0,
}),
```

Status caveat:
- **The hot path uses hyper, which handles padding correctly.** Our `lb-h2::frame` decoder appears to be unused in live data-plane code (`grep` for `lb_h2::decode_frame` returns only the test module). The crate's `pub fn decode_frame` is part of our public surface and would be wrong if anyone (us, downstream consumer, future refactor) wired it onto a live socket.
- This is a **lesson-not-yet-paid-for** finding (per task brief criterion 3): the bug is in our code waiting; we haven't hit it because the code is test-only.

Impact:
- If `lb-h2::decode_frame` is ever wired live (e.g., for a future native-H2 server fallback, or a fuzz harness that feeds the decoder real wire bytes), every PADDED frame will desync the parser. Subsequent frames are parsed from the middle of the previous frame's padding.
- A maliciously-crafted client that always sets PADDED on HEADERS makes our HpackBombDetector ratio calculation wrong (we count the wrong bytes as the encoded header block).
- HAProxy paid for this exact bug — `mux-h2: Properly consume padding for DATA frames` is in their CHANGELOG.

Reproduction:
```rust
// Build a DATA frame with PADDED set and verify the payload slice.
let mut buf = Vec::new();
buf.extend_from_slice(&[0, 0, 5]);     // length = 5 (1 pad-len + 3 data + 1 pad)
buf.push(0x00);                        // type = DATA
buf.push(0x08);                        // flags = PADDED
buf.extend_from_slice(&[0, 0, 0, 1]);  // stream_id = 1
buf.extend_from_slice(&[1, b'h', b'i', b'!', 0]);  // pad_len=1, "hi!", pad=0
let (frame, _) = lb_h2::decode_frame(&buf, 16384).unwrap();
if let lb_h2::H2Frame::Data { payload, .. } = frame {
    assert_eq!(&payload[..], b"hi!");   // FAILS — current behaviour returns [1,b'h',b'i',b'!',0]
}
```

Recommendation:
1. In `decode_frame_low`, check `flags & FLAG_PADDED != 0` for DATA and HEADERS. If set, read `pad_len = payload[0] as usize`, return `Err` if `pad_len + 1 > payload.len()`, slice `payload[1 .. payload.len() - pad_len]`.
2. Same for the HEADERS arm — combined with the existing `FLAG_PRIORITY_FLAG` handling, order matters: PADDED bytes wrap the entire (priority + headers) payload.
3. CONTINUATION does NOT have a PADDED flag (RFC 9113 §6.10) — leave that arm as-is, but add a code comment.
4. Backfill proptest cases for PADDED bit set with random pad lengths.
