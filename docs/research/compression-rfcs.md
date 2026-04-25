# Content Compression RFCs (gzip, deflate, brotli, zstd)

## Summary

Four compression algorithms are standardised for `Content-Encoding`
negotiation on HTTP: gzip (RFC 1952), raw DEFLATE (RFC 1951), Brotli
(RFC 7932), and Zstandard (RFC 8478, with zstd the IANA-registered
HTTP content-coding name). Negotiation follows RFC 9110 §12 (the
`Accept-Encoding` header with q-values) — RFC 7231 §5.3.4 is the
superseded-but-widely-cited predecessor and defines the same
syntax. ExpressGateway consolidates all four in the `lb-compression`
crate: streaming `Compressor` and `Decompressor` with a shared
`Algorithm` enum, a `transcode` helper that rewrites one coding into
another without materialising the plaintext, a `BreachGuard` for the
BREACH side-channel mitigation (CVE-2013-3587), and a `negotiate`
function that implements RFC 9110 §12.5.3 q-value ordering. Every
decompression path MUST be protected against compression bombs; we
enforce both an absolute byte ceiling (`max_bytes`) and an expansion
ratio (`max_ratio`).

## Scope in ExpressGateway

- `crates/lb-compression/src/lib.rs` — entire surface.
  - `Algorithm` enum at `lib.rs:67` (Gzip, Deflate, Brotli, Zstd).
  - `Compressor::new:123`, `Compressor::compress:162`,
    `Compressor::finish:191`.
  - `Decompressor::new:222`, `Decompressor::decompress:240`.
  - `decompress_read:276` — shared streaming reader with per-chunk
    bomb checks.
  - `transcode:334` — streaming re-encoding between algorithms.
  - `BreachGuard::should_compress:429` — stateless BREACH gate.
  - `negotiate:445` — `Accept-Encoding` parser with q-values and
    server tie-break.
  - `parse_qvalue:509` — rejects NaN/Infinity, clamps to [0,1].
  - `server_preference:497` — zstd > br > gzip > deflate.
- Tests: `tests/compression_zstd.rs`, `tests/compression_brotli.rs`,
  `tests/compression_gzip.rs`, `tests/compression_deflate.rs`,
  `tests/compression_transcode.rs`, `tests/compression_bomb_cap.rs`,
  `tests/compression_breach_posture.rs`.

## MUST clauses we implement

- **gzip wire format (RFC 1952 §2)** — Implemented via `flate2`
  (`CompressorInner::Gzip` at `lib.rs:98`) which emits the 10-byte
  header (`ID1=0x1f`, `ID2=0x8b`, `CM=8`), optional XFL/OS bytes, the
  DEFLATE payload, and an 8-byte footer (CRC-32 of uncompressed data
  and ISIZE modulo 2^32). CRC validation MUST be performed by the
  decompressor; `flate2::read::GzDecoder` does so and the error is
  surfaced as `DecompressFailed`.
- **DEFLATE block structure (RFC 1951 §3)** — Raw deflate streams
  carry no gzip header or trailer. Used when Content-Encoding is
  literally `deflate`. (A pitfall: some clients expect zlib-wrapped
  deflate per RFC 1950; ExpressGateway emits raw deflate per the
  HTTP/1.1 usage, matching RFC 9110 §8.4.1.3.)
- **Brotli wire format (RFC 7932)** — Implemented via `brotli`
  crate. Quality parameter comes from `Algorithm::default_level` =
  4 (a middle-of-the-road quality for proxy hot paths) at
  `lib.rs:87`. Window bits fixed at 22 (4 MiB) at `lib.rs:142`
  matching the upper bound in §9.1 that any decoder MUST accept.
- **Zstandard frame format (RFC 8478 / now RFC 8878)** — Implemented
  via `zstd` crate. Default level 3 at `lib.rs:87`. Frame magic
  `0xFD2FB528` and the optional content-size field are handled inside
  the library. The `Content-Encoding: zstd` registration is governed
  by RFC 8878 §5; ExpressGateway emits plain zstd frames (no
  dictionary).
- **Content-Encoding identifiers (RFC 9110 §8.4.1)** — Only `gzip`,
  `deflate`, `br`, and `zstd` are matched. Unknown encodings cause
  `negotiate` to skip the entry. Case-insensitivity is enforced by
  `to_ascii_lowercase` at `lib.rs:456`.
- **Accept-Encoding q-value semantics (RFC 9110 §12.5.3)** —
  `negotiate` parses each comma-separated token, extracts an
  optional `q=` parameter, and picks the highest-q supported
  algorithm. A token with `q=0` MUST be rejected as "not
  acceptable"; handled at `lib.rs:468`. `identity` with q>0 returns
  `None` (meaning "send uncompressed"). Regression:
  `negotiate_picks_highest_q` at `lib.rs:620`.
- **Server preference tie-break (RFC 9110 §12.5.3 "the server MAY
  choose")** — When two algorithms share the highest q-value, the
  proxy prefers zstd > br > gzip > deflate (`server_preference` at
  `lib.rs:497`). Regression:
  `negotiate_server_preference_tiebreak` at `lib.rs:650`.
- **q-value sanitisation** — `parse_qvalue` rejects `NaN`,
  `Infinity`, and `-Infinity` (`lib.rs:515`), clamps values above 1.0
  down to 1.0 per the BNF in §12.4.2, and accepts only finite floats.
  Regression: `negotiate_rejects_nan_and_inf` at `lib.rs:637`.
- **Decompression-bomb cap (absolute bytes)** — `max_bytes` is
  checked after every read chunk at `lib.rs:298`; exceeding it
  returns `CompressionError::OutputTooLarge`. This defends against
  the classic "42.zip" pathology.
- **Decompression-bomb cap (ratio)** — `max_ratio` is checked when
  `compressed_len > 0` (`lib.rs:303`); exceeding the ratio returns
  `CompressionError::BombDetected`. Regression:
  `bomb_detection_ratio` at `lib.rs:582`.
- **BREACH mitigation (CVE-2013-3587)** — When a response carries
  secret-bearing headers (e.g. `Set-Cookie`) AND the request could
  contain attacker-controlled reflected input, compression MUST be
  suppressed. `BreachGuard::should_compress:429` returns `false`
  only in that combined case; regression: `breach_guard_logic` at
  `lib.rs:611`.
- **Streaming API contract** — Neither the compressor nor the
  decompressor materialises the full plaintext; `decompress_read`
  reads in 8 KiB chunks and `transcode` pumps those chunks directly
  into the target encoder (`lib.rs:351`), so the largest in-flight
  buffer is one read-buffer's worth plus codec internal state.

## Edge cases & security

- **Compression bomb ("42.zip")** — A small compressed input that
  decodes to a huge output. Double-guarded by absolute size and
  ratio; regression in `tests/compression_bomb_cap.rs`.
- **BREACH / CRIME family** — `BreachGuard` implements the
  canonical decision rule (secret + reflected → do not compress);
  regression `tests/compression_breach_posture.rs`.
- **Malformed frames** — All four decoders surface I/O errors via
  `CompressionError::DecompressFailed`. A gzip stream with a bad
  CRC, a brotli stream with a truncated meta-block, or a zstd frame
  with a corrupt checksum are all caught.
- **Zero-length compressed input** — `compressed_len > 0` gate at
  `lib.rs:303` avoids division-by-zero in the ratio check.
- **Double-encoding** — If the upstream already set
  `Content-Encoding`, the proxy MUST NOT re-compress (PROMPT.md
  §22). Negotiation skips when Content-Encoding is already present.
- **Content-Type allowlist** — Only compressible MIME types listed in
  PROMPT.md §22 are eligible. Binary types (images, video) are left
  alone, avoiding wasted CPU for no gain.
- **Transcode DoS** — `transcode` applies the same bomb guards to
  the intermediate plaintext (`lib.rs:368-383`), so re-encoding
  gzip→zstd cannot be weaponised.
- **Huffman/Deflate quirk** — RFC 1951 allows stored blocks (no
  compression); a degenerate encoder could inflate output by ~5
  bytes per 64 KiB block. Our `max_bytes` cap defuses this on the
  decode side.
- **gRPC-encoding coupling** — gRPC's `grpc-encoding` (gzip,
  deflate, identity) shares this crate with HTTP content-encoding;
  see `grpc.md`.

## Known deviations / TODO

- **`zstd` dictionaries** — Not used. RFC 8878 allows dictionaries but
  HTTP-level content negotiation does not carry one, so plain frames
  are emitted.
- **Brotli dictionary** — Uses the standard static dictionary (RFC
  7932 Annex A); no custom dictionary support.
- **`Accept-Encoding: *`** — Not yet handled as a wildcard; a request
  with `Accept-Encoding: *` currently falls through because `*` is
  not one of the matched algorithm names. RFC 9110 §12.5.3 states
  `*` means "any content-coding not explicitly listed". TODO:
  treat `*` with positive q as a fall-through acceptance.
- **Snappy, LZ4, xz, compress (RFC 7230 legacy)** — Not supported.
- **Streaming encoder backpressure** — The `compress` method buffers
  until internal flush; a fully streaming "output-by-chunk" API is
  not yet exposed for HTTP/1.1 chunked body re-encoding.
- **Transfer-Encoding** — `Transfer-Encoding: gzip` is legal per
  RFC 9112 but we only implement Content-Encoding; Transfer-Encoding
  at the hop level stays as chunked or identity.

## Sources

- RFC 1952 (gzip) — <https://www.rfc-editor.org/rfc/rfc1952>
- RFC 1951 (DEFLATE) — <https://www.rfc-editor.org/rfc/rfc1951>
- RFC 1950 (zlib wrapper, referenced but not emitted) —
  <https://www.rfc-editor.org/rfc/rfc1950>
- RFC 7932 (Brotli) — <https://www.rfc-editor.org/rfc/rfc7932>
- RFC 8478 (Zstandard, obsoleted by 8878) —
  <https://www.rfc-editor.org/rfc/rfc8478>
- RFC 8878 (Zstandard, current) —
  <https://www.rfc-editor.org/rfc/rfc8878>
- RFC 9110 §8.4 Content-Encoding and §12.5.3 Accept-Encoding —
  <https://www.rfc-editor.org/rfc/rfc9110>
- CVE-2013-3587 (BREACH) —
  <https://nvd.nist.gov/vuln/detail/CVE-2013-3587>
- Zstd content-coding registration —
  <https://www.iana.org/assignments/http-parameters/http-parameters.xhtml#content-coding>
