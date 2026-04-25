# gRPC over HTTP/2 and HTTP/3

## Summary

gRPC is an RPC framework defined by the grpc.io specification documents
PROTOCOL-HTTP2.md and the gRPC status-code mapping. It is layered on
HTTP/2 (and, via the same framing, HTTP/3) and adds three primitives:
length-prefixed message framing, a status code carried in response
trailers (`grpc-status`, `grpc-message`), and a request deadline
carried in the `grpc-timeout` header. ExpressGateway implements these
in the `lb-grpc` crate: a fixed 5-byte frame header codec
(`GrpcFrame`, `encode_grpc_frame`, `decode_grpc_frame`), a complete
0..=16 status-code enum with bidirectional HTTP↔gRPC translation
(`GrpcStatus`), a `grpc-timeout` parser/formatter with all six time
units (`GrpcDeadline`), and a streaming-mode enumeration
(`StreamingMode`). The transport is HTTP/2 `lb-h2` or HTTP/3 `lb-h3`;
gRPC-aware compression defers to `lb-compression` while only gzip,
deflate, and identity are valid `grpc-encoding` values. The proxy
treats trailers as first-class and preserves `grpc-status`,
`grpc-message`, and `grpc-status-details-bin` across protocol hops.

## Scope in ExpressGateway

- `crates/lb-grpc/src/lib.rs` — public surface (`GrpcFrame`,
  `GrpcStatus`, `GrpcDeadline`, `StreamingMode`, `GrpcError`).
- `crates/lb-grpc/src/frame.rs` — length-prefixed framing.
  - `GRPC_HEADER_SIZE = 5` at `frame.rs:16`.
  - `DEFAULT_MAX_MESSAGE_SIZE = 4 MiB` at `frame.rs:29` (matches
    grpc-go / grpc-java defaults).
  - `decode_grpc_frame:44` — validates compressed flag (0 or 1),
    reads big-endian 4-byte length, enforces `max_message_size`.
  - `encode_grpc_frame:99` — 1-byte flag + 4-byte length + payload.
- `crates/lb-grpc/src/status.rs` — status codes.
  - `GrpcStatus` enum with 17 variants at `status.rs:10`.
  - `to_http_status:81` (gRPC→HTTP) and `from_http_status:102`
    (HTTP→gRPC).
- `crates/lb-grpc/src/deadline.rs` — `grpc-timeout` header.
  - `parse_timeout:29` accepting `H`, `M`, `S`, `m`, `u`, `n` units.
  - `format_timeout:66` picking the longest unit that represents the
    value exactly.
  - `remaining:88` for deadline propagation.
- `crates/lb-grpc/src/streaming.rs` — four streaming modes.
- `tests/grpc_unary.rs`, `tests/grpc_server_streaming.rs`,
  `tests/grpc_client_streaming.rs`, `tests/grpc_bidi_streaming.rs`,
  `tests/grpc_deadline.rs`, `tests/grpc_status_translation.rs`.

## MUST clauses we implement

- **5-byte length-prefix framing (gRPC PROTOCOL-HTTP2 §Data Frames)**
  — Every gRPC message on the wire is prefixed by 1 compressed-flag
  byte + 4 big-endian length bytes, followed by that many payload
  bytes. `decode_grpc_frame` enforces this exactly
  (`frame.rs:48-89`), rejecting:
  - any buffer shorter than 5 bytes with `GrpcError::Incomplete`,
  - any compressed-flag byte not in `{0, 1}` with
    `GrpcError::InvalidFrame` (`frame.rs:57`),
  - any declared length exceeding `max_message_size` with
    `GrpcError::MessageTooLarge` (`frame.rs:73`).
- **Content-Type recognition** — A request is a gRPC request iff
  `Content-Type` begins with `application/grpc` (spec: "Content-Type:
  application/grpc[+proto|+json|...]"). PROMPT.md §13 item 1 captures
  this; detection happens in the pipeline before frames reach
  `lb-grpc`.
- **Status code enumeration (0..=16)** — `GrpcStatus::from_code` is
  exhaustive over the 17 canonical codes and rejects anything outside
  the range with `GrpcError::InvalidStatus` (`status.rs:54`).
  Regression: `status_from_code_invalid` at `lib.rs:129`.
- **Status code trailer** — gRPC responses MUST convey status via
  `grpc-status` in trailer HEADERS (§Responses). Our trailer-aware
  proxying in `lb-h2` preserves trailing HEADERS frames verbatim so
  `grpc-status`, `grpc-message`, and `grpc-status-details-bin` never
  drop.
- **HTTP↔gRPC status translation** — gRPC spec §HTTP-to-Status-Code
  table is implemented bidirectionally:
  - `to_http_status`: maps gRPC→HTTP (e.g. OK→200, DeadlineExceeded
    →504, Unauthenticated→401, Unavailable→503).
  - `from_http_status`: maps HTTP→gRPC (400|500→Internal,
    401→Unauthenticated, 429|502..=504→Unavailable).
  Regression at `lib.rs:135-151`.
- **`grpc-timeout` header format (§grpc-timeout)** — `Timeout = Digits
  Unit` where Unit ∈ {H, M, S, m, u, n}. `parse_timeout` splits the
  last byte as the unit, parses the digits as a `u64`, and performs
  unit conversion with saturating multiplication:
  - `H` → `*3_600_000` ms
  - `M` → `*60_000` ms
  - `S` → `*1_000` ms
  - `m` → identity (ms)
  - `u` → ceiling-divide by 1_000
  - `n` → ceiling-divide by 1_000_000.
  Ceiling division is deliberate: a non-zero sub-millisecond timeout
  MUST NOT round to 0 ms, because the gRPC spec requires that a
  positive timeout is respected. Regression:
  `deadline_submillisecond_rounds_up` at `lib.rs:206`.
- **Deadline propagation** — The proxy SHOULD forward `grpc-timeout`
  to the backend reduced by time already elapsed. `GrpcDeadline::
  remaining` returns `None` when the deadline is already exceeded
  (`deadline.rs:88`), at which point the proxy must cancel the stream
  with `DEADLINE_EXCEEDED (4)`.
- **Maximum message size MUST be enforced before allocation** — The
  length-before-allocate ordering at `frame.rs:73` ensures a
  maliciously large length field fails with
  `GrpcError::MessageTooLarge` without allocating the claimed buffer.
  Regression: `grpc_message_too_large` at `lib.rs:222`.
- **Streaming modes (§Streaming)** — Four modes (Unary, Server,
  Client, Bidi) represented in `StreamingMode`. The proxy treats
  every stream as potentially bidirectional at the H2/H3 layer;
  regression tests cover all four (`tests/grpc_unary.rs`,
  `tests/grpc_server_streaming.rs`, `tests/grpc_client_streaming.rs`,
  `tests/grpc_bidi_streaming.rs`).
- **Trailers-only response** — When the server has only a status to
  report (e.g. immediate error), gRPC permits a single HEADERS frame
  carrying both response headers and trailers. The proxy preserves
  this with END_STREAM on the HEADERS frame.
- **`grpc-encoding` negotiation** — Valid values are `identity`,
  `gzip`, `deflate`; other codecs require explicit client/server
  agreement. The proxy defers compression handling to
  `lb-compression`; if `grpc-encoding: identity` is requested, the
  proxy MUST NOT apply compression (PROMPT.md §13 "gRPC-Aware
  Compression").

## Edge cases & security

- **Message size DoS** — `MessageTooLarge` is returned before the
  body is read; the length field is a 32-bit BE integer so the
  maximum claimable size is 4 GiB. The default cap (4 MiB) matches
  grpc-go/grpc-java.
- **Compressed-flag fuzz** — The flag byte MUST be exactly 0 or 1;
  any other value is rejected. Test
  `frame_incomplete_header` at `lib.rs:74` and the validation at
  `frame.rs:57` lock this down.
- **Trailer injection / smuggling** — The proxy requires that
  `grpc-status` appear in trailers, never in initial headers (unless
  it is a trailers-only response). The HPACK path through `lb-h2`
  preserves trailer HEADERS framing.
- **Deadline attack** — Attacker sets `grpc-timeout: 99999999999H`.
  Saturating multiplication in `parse_timeout` caps the result at
  `u64::MAX`; the proxy then clamps to its configured maximum (300 s
  per PROMPT.md §13).
- **Status range attack** — A forged trailer carrying `grpc-status:
  99` is rejected by `GrpcStatus::from_code` as `InvalidStatus(99)`;
  the proxy reports the backend as faulty.
- **Invalid timeout format** — `""`, `"S"`, `"abc"`, `"5x"` all
  return `InvalidTimeout`; test `deadline_invalid_format` at
  `lib.rs:198`.
- **Bidirectional streaming** — H2/H3 flow-control MUST be applied
  per direction. The proxy propagates backpressure so a slow server
  does not starve the client-side DATA frames.
- **Cancellation** — Stream RST_STREAM from either side MUST translate
  to the `CANCELLED (1)` gRPC status (`tests/grpc_status_translation.rs`).

## Known deviations / TODO

- **Snappy / custom codecs** — Only gzip, deflate, and identity are
  supported; the spec allows `application/grpc+*` with custom codecs
  but the proxy strips unknown `grpc-encoding` values and falls back
  to identity.
- **Reflection service** — Not proxied specially; gRPC reflection
  works transparently because it is a normal unary/streaming RPC.
- **gRPC-Web** — Not yet bridged; the proxy does not currently
  translate `Content-Type: application/grpc-web` to
  `application/grpc` on the backend side.
- **Health check endpoint (`grpc.health.v1.Health/Check`)** — The
  proxy does not synthesise this response; it forwards to the backend
  (PROMPT.md §13 "gRPC Health Check" marks this as future work).
- **`grpc-status-details-bin`** — Forwarded verbatim but not parsed
  or validated; this is per-spec (the proxy is status-opaque).
- **Retry / hedging policies (A6 / A31)** — Out of scope; the proxy
  is stateless with respect to gRPC-level retries.

## Sources

- gRPC HTTP/2 protocol — <https://github.com/grpc/grpc/blob/master/doc/PROTOCOL-HTTP2.md>
- gRPC status codes — <https://github.com/grpc/grpc/blob/master/doc/statuscodes.md>
- gRPC status-to-HTTP mapping —
  <https://github.com/grpc/grpc/blob/master/doc/http-grpc-status-mapping.md>
- gRPC compression —
  <https://github.com/grpc/grpc/blob/master/doc/compression.md>
- gRPC health checking —
  <https://github.com/grpc/grpc/blob/master/doc/health-checking.md>
- RFC 9113 (HTTP/2 transport) — <https://www.rfc-editor.org/rfc/rfc9113>
