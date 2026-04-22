# HPACK (RFC 7541) and QPACK (RFC 9204)

## Summary

HPACK (RFC 7541) is the header compression format for HTTP/2, and QPACK
(RFC 9204) is the HTTP/3 successor that keeps HPACK's core ideas but
makes the dynamic table update path order-independent on an unordered
QUIC stream. ExpressGateway implements both: HPACK in
`crates/lb-h2/src/hpack.rs` (full 61-entry static table, dynamic table
with eviction, integer coding with 4/5/6/7-bit prefixes, literal
representations, and size updates), and QPACK in
`crates/lb-h3/src/qpack.rs` (99-entry static table, literal-only
encoding; dynamic table is optional per §4.1.1 and is disabled).
Decompression-bomb protection is mandatory — `HpackBombDetector` and
`QpackBombDetector` cap both decoded/encoded byte ratio and absolute
decoded size. Huffman coding is intentionally not emitted on the write
path; the H-bit is always 0, which is a fully compliant optional
subset per RFC 7541 §5.2.

## Scope in ExpressGateway

- `crates/lb-h2/src/hpack.rs` — HPACK codec.
  - `STATIC_TABLE` (61 entries, 1-indexed) at `hpack.rs:18`.
  - `encode_integer:263` / `decode_integer:284` — N-bit prefix integer
    codec per RFC 7541 §5.1.
  - `encode_string:311` / `decode_string:317` — string-literal codec
    (always H=0 on emit; tolerant of H=1 on read).
  - `DynamicTable` implementation with eviction when size exceeds
    `set_max_size` (see `hpack.rs:200`).
  - `HpackEncoder::encode:353` / `HpackDecoder::decode:409`.
  - `table_get:206` — combined static + dynamic lookup with index
    validation.
- `crates/lb-h3/src/qpack.rs` — QPACK codec.
  - `STATIC_TABLE` (99 entries, 0-indexed) starting `qpack.rs:12`.
  - Encoder/decoder pair (literal-only; field-line representations only).
- `crates/lb-h2/src/security.rs` — `HpackBombDetector::check:185`.
- `crates/lb-h3/src/security.rs` — `QpackBombDetector::check:35`.
- `tests/security_hpack_bomb.rs`, `tests/security_qpack_bomb.rs`.

## MUST clauses we implement

- **HPACK static table is immutable and 1-indexed (RFC 7541 Appendix
  A)** — Our table at `hpack.rs:18` begins with a sentinel at index 0
  and runs through index 61 (`www-authenticate`). Index 0 is rejected
  by `table_get:207` with "index 0 is invalid", matching §2.3.2.
- **Indexed header field representation (§6.1)** — First bit 1; 7-bit
  prefix integer identifies the absolute index. Encoder emits
  `0x80 | index` via `encode_integer(..., 7, 0x80)` at `hpack.rs:360`.
- **Literal with incremental indexing (§6.2.1)** — First two bits `01`;
  6-bit prefix integer for name-index (0 means "new name, literal
  follows"). Implemented at `hpack.rs:362,370`.
- **Literal without indexing (§6.2.2) and never indexed (§6.2.3)** —
  First four bits `0000` / `0001`; 4-bit prefix. Decoder branches at
  `hpack.rs:445`. "Never indexed" is honoured on decode (the value is
  not inserted into the dynamic table) but our encoder never emits it
  by default — this is legal per §6.2.3, which leaves the choice to
  the encoder.
- **Dynamic table size updates (§6.3)** — First three bits `001`;
  5-bit prefix. Decoder handles at `hpack.rs:467`; encoder calls
  `set_max_table_size` to propagate SETTINGS_HEADER_TABLE_SIZE.
- **Integer encoding respects prefix length (§5.1)** — Values below
  `2^N - 1` fit in the prefix byte; larger values overflow into 7-bit
  continuation bytes with MSB=1 on all but the last (`encode_integer`
  loop at `hpack.rs:268-279`).
- **Decoder MUST reject integer overflow (§5.1)** — `decode_integer`
  bails with `H2Error::HpackError("integer overflow")` once the
  shift exponent `m > 28` (`hpack.rs:303`), preventing attacker-sent
  varints from consuming unbounded bytes.
- **Dynamic-table eviction (§4.3)** — Entries are evicted FIFO until
  the table size (name length + value length + 32 octets per entry,
  §4.1) fits the limit; implemented in `DynamicTable::evict`.
- **HPACK bomb prevention** — Not a MUST in RFC 7541 but mandated by
  RFC 7541 §7 security considerations and by CVE-2016-6525.
  `HpackBombDetector::check` rejects when decoded size exceeds
  `max_decoded_size` or when `decoded / encoded > max_ratio`
  (`security.rs:185`).
- **QPACK static table is 0-indexed (RFC 9204 Appendix A)** — Our
  table at `qpack.rs:12` starts with `:authority` at index 0, which
  matches the RFC (HPACK's 1-based scheme was explicitly redesigned
  for QPACK).
- **QPACK "encoded field section" header** — Begins with a Required
  Insert Count and Delta Base both encoded as QPACK varints (RFC 9204
  §4.5.1). Because we disable the dynamic table, both are emitted as
  0 and the decoder validates that the Required Insert Count is zero.
- **QPACK MUST NOT reference dynamic entries that haven't been
  inserted (§2.1.1.1)** — Trivially satisfied: we never insert.
- **Section-acknowledgement streams** — QPACK requires unidirectional
  encoder/decoder streams (§4.2). Since the dynamic table is unused,
  the only bytes on those streams are SETTINGS acknowledgements,
  which the proxy emits through the `lb-quic` control path.
- **Cookie header compaction for QPACK** — RFC 9204 §4.2.1 inherits
  RFC 9113 §8.2.3 (cookie crumb concatenation). Our encoder emits each
  `cookie` value as a distinct field line, which decoders MUST
  concatenate with `"; "` before forwarding.
- **QPACK / HPACK bomb detection** — `QpackBombDetector::check:35`
  mirrors the HPACK detector.

## Edge cases & security

- **HPACK bomb (CVE-2016-6525 family)** — Attacker sends a tiny
  compressed block that decodes into an enormous header list via
  repeated references to a short dynamic-table entry. `HpackBombDetector`
  enforces both an absolute cap and a ratio cap; regression at
  `tests/security_hpack_bomb.rs`.
- **QPACK bomb** — Analogous; regression
  `tests/security_qpack_bomb.rs`. Even though we do not populate a
  dynamic table on the encode side, a decoder must still detect
  malicious encoders — hence the ratio guard on the wire bytes vs.
  cumulative decoded size.
- **Integer-decoding OOM** — RFC 7541 §5.1 does not impose a varint
  length bound, making it a natural DoS target. The `m > 28` overflow
  check at `hpack.rs:303` caps the read at five bytes plus the prefix
  byte (since 2^28 already exceeds any sensible header-count limit).
- **String-decoding OOM** — String lengths are decoded with the same
  prefix integer codec, then validated against the remaining buffer
  via bounds-checked `.get(start..end)?` at `hpack.rs:321`. No
  pre-allocation.
- **Non-UTF-8 header bytes** — `decode_string` enforces UTF-8
  (`hpack.rs:323`). RFC 7541 §5.2 allows opaque bytes; we are stricter,
  which matches the RFC 9110 §5.5 "field values SHOULD be ASCII" and
  blocks NULs and obsolete bytes in header values.
- **Huffman** — The decoder tolerates H=1 as "best-effort raw" per
  the hpack module comment. Strict RFC compliance requires a full
  Huffman table; the current behaviour is a known limitation (see
  TODO).
- **0-RTT header leakage via QPACK dynamic table** — Avoided by
  disabling the dynamic table entirely; there is no cross-stream
  state.

## Known deviations / TODO

- **Huffman coding is not emitted** (H=0 only). Compliant for
  senders per §5.2. Receivers with H=1 are handled best-effort
  rather than decoded, which is stricter than the RFC requires —
  some peers may refuse to interop until a full Huffman decoder is
  added.
- **QPACK dynamic table** — Not implemented. We advertise
  `SETTINGS_QPACK_MAX_TABLE_CAPACITY=0`, which is explicitly allowed.
- **QPACK blocked-stream tracking** — Not implemented; follows from
  the dynamic-table decision.
- **Literal-never-indexed** — Never emitted on our write path; the
  field names that the RFC calls out as sensitive (authorization,
  cookie, set-cookie) would benefit from this representation. TODO.
- **Dynamic table size synchronisation** — We accept SIZE_UPDATE
  frames on the wire but do not currently propagate them to the
  encoder side of the symmetric HPACK codec unless
  `set_max_table_size` is called explicitly.

## Sources

- RFC 7541 (HPACK) — <https://www.rfc-editor.org/rfc/rfc7541>
- RFC 7541 errata — <https://www.rfc-editor.org/errata/rfc7541>
- RFC 9204 (QPACK) — <https://www.rfc-editor.org/rfc/rfc9204>
- RFC 9204 errata — <https://www.rfc-editor.org/errata/rfc9204>
- CVE-2016-6525 (HPACK bomb) —
  <https://nvd.nist.gov/vuln/detail/CVE-2016-6525>
- "HTTP/2 Rapid Reset + HPACK bomb combinations" —
  <https://blog.cloudflare.com/technical-breakdown-http2-rapid-reset-ddos-attack/>
