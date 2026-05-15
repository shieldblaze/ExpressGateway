### ROUND8-L7-02 — Chunk-size hex parser accepts `+`, leading whitespace, unbounded digits (nginx CVE-2013-2028 / hyper GHSA-5h46-h7hh-c6x9 / HAProxy `h1_append_chunk_size` class)

Reference: `audit/round-8/research/nginx.md` lesson 1 (CVE-2013-2028, RCE-territory stack overflow on chunked); `audit/round-8/research/hyper-h2-quinn.md` lesson 2 (GHSA-5h46-h7hh-c6x9, integer overflow on chunk size); `audit/round-8/research/haproxy.md` lesson 3 (`BUG/MAJOR: mux_h1: fix stack buffer overflow in h1_append_chunk_size`). Three big proxies took the *same* primitive.
Our equivalent: `crates/lb-h1/src/chunked.rs:127-158` (`try_read_size`)

Severity: high
Status:   Verified-Fixed(verifier=verify, 2cc69ba0)   <!-- INDEPENDENT VERIFICATION (verifier=verify, author=div-l7): hand-rolled parse_chunk_size_hex rejects empty/len>16/non-hex/+/-/ws/0x/unicode-digits; checked_shl; raw-byte slice no trim; usize::try_from 32-bit guard. round8_chunk_size_cve_corpus 14/14 PASS. No bypass found. See audit/round-8/verify/l7.md. -->

Divergence:
- **Reference** (collectively): chunk-size hex parser must (a) reject leading sign / whitespace, (b) enforce a hard cap of 16 hex digits for u64 (i.e. assume worst-case), (c) detect overflow on accumulate rather than silently truncate.
- **Us**: `usize::from_str_radix(hex_trimmed, 16)`. Verified at runtime:
  ```
  usize::from_str_radix("+5".trim(), 16)         = Ok(5)              // sign accepted
  usize::from_str_radix("  5".trim(), 16)        = Ok(5)              // we strip whitespace first via .trim()
  usize::from_str_radix("00000…00ff", 16)        = Ok(255)            // unlimited leading zeros
  ```
  The `.trim()` on `hex_part` also strips inner-line whitespace silently. There is no digit-count cap; an attacker can submit a 4 KiB chunk-size line of leading zeros without the parser rejecting it. On a 32-bit platform `usize::from_str_radix(..., 16)` is u32, which is *smaller* than u64 → off-by-one across the proxy/backend on chunk sizes 0x1_0000_0000..=0xFFFF_FFFF_FFFF_FFFF.
- Existing `H1Error::InvalidChunkEncoding` branch is reached only by the `parse()` returning `Err` (overflow), but accepting `+5` is silent.

Impact:
- Cross-proxy smuggling primitive. If our proxy parses `chunk-size: +5\r\n` and forwards verbatim, a strict upstream (e.g., a hardened nginx fronted by us) sees the line as malformed and either rejects (mismatched response queue) or interprets a different chunk boundary. Wave-2 of the smuggling matrix did not cover this input.
- 32-bit usize platform: `\r\nFFFFFFFFFFFFFFFF\r\n` (16 hex digits, 64-bit max) overflows the parser to error; we do reject — good. But on 32-bit a value like `0x1_0000_00FF` parses cleanly as 0xFF after truncation. Even on 64-bit, the spec says the chunk-size is unbounded; we should cap at usize::MAX and disagree gracefully.
- The `.trim()` is the same primitive HAProxy `BUG/MAJOR: h1_append_chunk_size` defended against — extra whitespace inside the size line shifts framing.

Reproduction:
- Unit case (would fail if rejected):
  ```
  let mut d = ChunkedDecoder::new();
  assert!(d.feed(b"+5\r\nhello\r\n0\r\n\r\n").is_err());  // currently passes (no err)
  let mut d = ChunkedDecoder::new();
  assert!(d.feed(b"00000000000005\r\nhello\r\n0\r\n\r\n").is_err());  // 14-digit cap is RFC-compliant?
  ```
- Behaviour confirmed by direct `rustc` smoke (see this finding's tool output capture).

Recommendation:
1. Replace `usize::from_str_radix(hex_trimmed, 16)` with a hand-rolled digit-only parser:
   - Reject `hex_part != hex_trimmed` (any whitespace in the size line is a smell — RFC 9112 §7.1 chunk-size = 1\*HEXDIG).
   - Reject zero-length size strings before any parsing.
   - Reject any byte outside `[0-9a-fA-F]` (catches `+`, `-`, ` `, `\t`).
   - Cap at 16 hex digits (u64); on 32-bit platforms cap at 8.
   - Accumulate via `checked_shl(4) + nibble`, error on `None`.
2. Add fuzz seeds: `+1`, ` 1`, `\t1`, `00000000000000000001`, `ffffffffffffffffff` (overflow), `1;ext=`.
3. Pin this in `proptest_parser.rs` — current proptest only checks "no panic", not "rejects ill-formed".
