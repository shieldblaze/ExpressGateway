# Plan for ROUND8-L7-02 — Chunked size-line lexer hardening

Finding-ref:   ROUND8-L7-02 (high, status: Open)
Files touched:
  - `crates/lb-h1/src/chunked.rs`                (`try_read_size`, line 127–158)
  - `crates/lb-h1/tests/proptest_parser.rs`      (extend with CVE-class seeds — see also L7-14)
  - new test file: `crates/lb-l7/tests/round8_chunked_lexer.rs` (integration shape)
  - new test file: `crates/lb-h1/tests/round8_chunk_size_cve_corpus.rs` (unit shape)

Approach (≤500 words):

Replace `usize::from_str_radix(hex_trimmed, 16)` with a hand-rolled
digit-only parser that mirrors **nginx CVE-2013-2028**, **hyper
GHSA-5h46-h7hh-c6x9**, and **HAProxy `BUG/MAJOR: mux_h1: fix stack
buffer overflow in h1_append_chunk_size`** rejection rules.

Concrete shape (in `lb-h1::chunked::try_read_size`):

```rust
// pseudo:
fn parse_chunk_size_hex(line: &[u8]) -> Result<u64, H1Error> {
    if line.is_empty() { return Err(H1Error::InvalidChunkEncoding); }
    if line.len() > 16 { return Err(H1Error::InvalidChunkEncoding); }  // u64 cap
    let mut value: u64 = 0;
    for &b in line {
        let nibble = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => return Err(H1Error::InvalidChunkEncoding),  // '+', '-', ' ', '\t', ';' here
        } as u64;
        value = value.checked_shl(4).ok_or(H1Error::InvalidChunkEncoding)?;
        value |= nibble;
    }
    Ok(value)
}
```

Caller-side changes (`try_read_size`):
1. After locating the CRLF, the *hex portion* is the bytes before the
   first `;` (chunk-extension separator per RFC 9112 §7.1.1). Do NOT
   `.trim()` — RFC 9112 ABNF for `chunk-size` is `1*HEXDIG`, no
   whitespace allowed inside or around.
2. Reject the whole line if the byte range from start-of-line to the
   `;` (or CRLF) contains anything outside the HEXDIG set.
3. Chunk extensions (`;ext=value`) MAY contain BWS per RFC 9112; we
   currently ignore them, which is correct — leave that pass-through.
4. Cap is **16 hex digits** to ensure u64 fits with no truncation.
   On 32-bit `usize`, additionally reject any value `> usize::MAX as
   u64` before casting; ChunkedDecoder must internally use `u64` so
   the framing layer matches the protocol regardless of pointer width.

Reference pattern: hyper's `parse_chunk_size` in `hyper/src/proto/h1/decode.rs` is the closest crate-mirror; it enforces the exact rules above. HAProxy's `h1_append_chunk_size` post-fix shape lifts the same idea (read the diff in their patch series).

Cross-with L7-14 (proptest seed extension): the new behaviour must be
asserted by named regression cases in
`tests/round8_chunk_size_cve_corpus.rs`:
- `b"+5\r\nhello\r\n0\r\n\r\n"` → `Err`
- `b"-5\r\n…"` → `Err`
- `b" 5\r\n…"` → `Err` (leading space)
- `b"5 \r\n…"` → `Err` (trailing space inside size token)
- `b"\t5\r\n…"` → `Err`
- `b"00000000000000005\r\nhello\r\n0\r\n\r\n"` → `Err` (17 hex digits)
- `b"ffffffffffffffff\r\n"` → `Ok(u64::MAX)` but caller must reject as
  exceeding `Http1Limits::max_body_size` (separate concern, already
  enforced upstream of the lexer)
- `b"5;ext=foo\r\nhello\r\n0\r\n\r\n"` → `Ok(5)` (extensions still allowed)

Proof:
  - `round8_chunk_size_cve_corpus::rejects_plus_sign` — invariant: `+5` chunk-size returns `H1Error::InvalidChunkEncoding`; no payload bytes consumed.
  - `round8_chunk_size_cve_corpus::rejects_leading_whitespace` — invariant: ` 5\r\n…` and `\t5\r\n…` reject.
  - `round8_chunk_size_cve_corpus::rejects_overlong_hex` — invariant: 17+ hex digits reject (even when the underlying numeric value fits in u64 after truncation).
  - `round8_chunk_size_cve_corpus::accepts_chunk_extensions` — invariant: `5;ext=foo` parses as size=5, preserves the extension bytes for the framer to discard.
  - `round8_chunk_size_cve_corpus::overflow_checked_shl` — invariant: `10000000000000000` (17 chars, value > u64::MAX) rejects via `checked_shl`, not silent truncation.
  - `round8_chunked_lexer::end_to_end_proxy_rejects_smuggled_chunk` — integration: the L7 proxy returns `400 Bad Request` (or equivalent) and does not forward the malformed chunked body upstream.

Risk / blast radius:
  - Tight, well-contained crate (`lb-h1`). The behavioural change is *stricter* than today; any client currently relying on accidental tolerance (e.g. a misbehaving toolchain that emits `+5` chunk-sizes) will now fail. We argue: any such client is already on borrowed time against a strict upstream and the LB *should* fail fast.
  - 32-bit platforms: we currently silently truncate via `usize`; the fix forces `u64` accumulation. Verify the framer's downstream consumers (`ChunkedDecoder::feed_chunk_data`) handle the type change without API churn. If `feed_chunk_data` keys on `usize`, retain a `usize::try_from(u64_size).map_err(...)?` at the boundary.

Cross-ref:
  - L7-14 (proptest seed extension) — this plan ships the seeds.
  - L7-03 (empty header name) and L7-04 (XFF iteration) share the
    "primitive-parser hardening" theme; sequence the three fixes as
    one PR if convenient, but plans remain separate.

**Audit-failure-mode theme:**
  - **Theme 4 — Multi-validator audit handoff**: CODE-2-11 shipped the
    proptest harness with a "no panic + bounded consumption" floor.
    The protocol validator was supposed to seed the harness with the
    CVE corpus and never did. This plan closes that handoff *and* the
    L7-14 plan structurally prevents recurrence by making the corpus
    seed-set a separate CI artefact.

Owner:           div-l7
Lead-approval: approved 2026-05-14 team-lead-r8
