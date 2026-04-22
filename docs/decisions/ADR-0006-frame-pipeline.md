# ADR-0006: HTTP frame pipeline — frame-by-frame, never buffer-then-parse

- Status: Accepted
- Date: 2026-04-22
- Deciders: ExpressGateway team
- Consulted: Cloudflare Pingora design notes, nginx HTTP/2 module source,
  Envoy codec architecture, RFC 9113 §10 (HTTP/2 security
  considerations), CVE-2023-44487 post-mortems.

## Context and problem statement

An L7 proxy reads bytes from a client, performs some decision-making, and
writes bytes to an upstream. For HTTP/1.1 this is byte-oriented; for
HTTP/2 it is frame-oriented; for HTTP/3 it is QUIC stream and DATAGRAM-
oriented. Two architectural shapes are possible for the codec layer:

- **Buffer-then-parse**: accumulate bytes until a complete logical unit
  (request, response, HEADERS + CONTINUATION sequence) is fully
  reassembled, parse it into a high-level type, then re-emit the bytes
  to the upstream.
- **Frame-by-frame**: on every frame-sized read, parse just enough
  header to route/decide, then forward the frame body as raw `Bytes`
  without materialising a logical message.

The tension is between semantic richness (buffer-then-parse is more
convenient for middleware that needs the full request) and adversary-
resistance (buffer-then-parse is where the flood-class CVEs live,
because "buffer" implies unbounded allocation). The 2023 Rapid-Reset
attack (CVE-2023-44487) and the 2024 CONTINUATION flood
(CVE-2024-24549) both exploited exactly this: a server that waits to see
"end of headers" before making a decision will happily allocate while
the attacker strings CONTINUATION frames together.

For a load balancer, a third consideration dominates: memory per
connection. At 100 000 concurrent connections, even a 64 KiB
per-connection reassembly buffer is 6.4 GiB of RAM. We cannot afford it.

## Decision drivers

- Worst-case memory must be bounded by (concurrent connections) ×
  (one-frame-worth-of-bytes), not by (concurrent connections) ×
  (one-request-worth-of-bytes).
- Backpressure must propagate from upstream to downstream at frame
  granularity.
- Flood-class attacks must be rejected at parse time, before allocation
  escalates.
- Forwarding-mode proxying should be close to zero-copy: the proxy is
  mostly moving bytes, not transforming them.
- The codec must still be able to *optionally* materialise a full
  request for L7 policies (WAF, HTTP header mutation) when the operator
  opts in.
- Lint-gate compatibility: no `unwrap()`, no unbounded allocation.

## Considered options

1. Buffer-then-parse — simpler to write, friendlier to middleware.
2. Frame-by-frame with optional accumulation at policy decision points.
3. Streaming parse with a bounded reassembly buffer and hard-cap reject.

## Decision outcome

Option 2. `lb-h2`, `lb-h1`, and `lb-h3` all implement frame-by-frame
(or chunk-by-chunk for HTTP/1) pipelines. The public API of `lb-h2`
exposes `H2Frame` and `decode_frame` / `encode_frame` directly
(`crates/lb-h2/src/lib.rs:21`), not a full request type; requests are
assembled on demand when a policy demands it.

The codec's `DEFAULT_MAX_FRAME_SIZE` constant is the hard cap — any
frame exceeding it is rejected before allocation.

## Rationale

- **Security**: the three `lb-h2` security detectors
  (`RapidResetDetector`, `ContinuationFloodDetector`, `HpackBombDetector`
  in `crates/lb-h2/src/security.rs`) are frame-granularity; they cannot
  function in a buffer-then-parse pipeline because by the time the
  buffer is full, it is too late.
- **Memory**: max per-connection working set is
  `DEFAULT_MAX_FRAME_SIZE` bytes — 16 KiB by RFC 9113 default — plus the
  HPACK dynamic table ceiling (another configurable 4 KiB by default).
  At 100 k connections that's 2 GiB, not 6.4 GiB; at 1 M connections
  still 20 GiB, which is the right order of magnitude.
- **Early rejection**: malformed frames (invalid type, reserved bit
  set, length exceeds `MAX_FRAME_SIZE`) are detected in the 9-byte
  frame header — `frame.rs` constant definitions `FRAME_DATA` … up to
  `FRAME_CONTINUATION` bound the valid type space.
- **Zero-copy forwarding**: `encode_frame` consumes a `Bytes` payload
  (see `crates/lb-h2/src/frame.rs`; the crate imports `Bytes, BytesMut`
  from the `bytes` crate). A frame decoded off the downstream socket
  can be re-encoded onto the upstream socket by cloning a reference-
  counted `Bytes` slice — no copy.
- **Backpressure**: `tokio::io::AsyncRead` returns whatever the kernel
  hands us; we parse exactly one frame header at a time from that
  buffer and pause if the body is not yet complete. This yields
  naturally at frame boundaries, which is the tokio scheduler's sweet
  spot.
- **Policy hook**: when an operator wants request-level mutation, the
  codec can buffer a bounded number of frames under an explicit
  per-connection cap, configured in `lb-config`. The default is "pass
  through," and the buffered mode is opt-in.

## Consequences

### Positive
- Memory is frame-bounded, not request-bounded.
- Security detectors have the granularity they need to reject floods.
- Forwarding is close to zero-copy via `bytes::Bytes`.
- Backpressure propagates correctly through the tokio `AsyncRead` /
  `AsyncWrite` stack.

### Negative
- Middleware that needs a fully materialised request (some WAF rules)
  must opt into buffered mode and accept the per-connection memory
  cost.
- Writing frame-by-frame parsers is harder than writing request-oriented
  parsers; state lives on the stack of the decoder function across
  async `.await` points, which is fiddly in Rust.

### Neutral
- The codec's public API (`H2Frame` rather than `Request<B>`) is more
  verbose to call but gives operators the knob they need.

## Implementation notes

- `crates/lb-h2/src/frame.rs` — frame header parsing; constants at the
  top enumerate RFC 9113 frame types.
- `crates/lb-h2/src/lib.rs:21` — re-exports `DEFAULT_MAX_FRAME_SIZE`,
  `H2Frame`, `decode_frame`, `encode_frame`.
- `crates/lb-h2/src/security.rs` — frame-level security detectors;
  operate on a stream of `(frame_type, flags, length)` events.
- `crates/lb-h1/src/lib.rs` — equivalent chunk-by-chunk pipeline for
  HTTP/1.1 (`Transfer-Encoding: chunked`), same philosophy.
- `crates/lb-h3/src/lib.rs` — frame-by-frame for HTTP/3's QUIC-stream
  frame layout.
- `tests/conformance_h1.rs`, `tests/conformance_h2.rs`,
  `tests/conformance_h3.rs` — conformance tests exercise the
  frame-by-frame paths.

## Follow-ups / open questions

- Expose a frame-accumulation policy in `lb-config` so operators can
  turn on buffered mode per-route, not globally.
- Consider adopting `http-body` v1 traits for the buffered-mode API
  once we need interop with external middleware.

## Sources

- RFC 9113 §10 — HTTP/2 security considerations.
- CVE-2023-44487 post-mortems (Cloudflare, Google, AWS).
- Cloudflare Pingora talk, "Pingora is open-source" (2024).
- Internal: `crates/lb-h2/src/{frame,security,lib}.rs`.
