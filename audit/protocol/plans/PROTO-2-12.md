# Plan for PROTO-2-12 — Trailer pass-through across H1↔H2/H3 (non-gRPC)

Finding-ref:    PROTO-2-12 (medium, Open)
Files touched:
  - `crates/lb-l7/src/lib.rs` (`BridgeResponse` adds
    `trailers: Option<HeaderMap>` and supports
    `http_body::Frame::Trailers` streaming) — **proto's bridge crate**
    per synthesis §D
  - `crates/lb-l7/src/h1_to_h2.rs`
  - `crates/lb-l7/src/h2_to_h1.rs`
  - `crates/lb-l7/src/h1_to_h3.rs`
  - `crates/lb-l7/src/h3_to_h1.rs`
  - `crates/lb-l7/src/h2_to_h2.rs`
  - `crates/lb-l7/src/h3_to_h3.rs`
  - `crates/lb-l7/src/h2_to_h3.rs`
  - `crates/lb-l7/src/h3_to_h2.rs`
  - `crates/lb-l7/src/util/hop.rs` (`StrippedTrailers` newtype per
    PROTO-2-07 cross-cut)
  - `tests/trailers_h1_to_h2.rs` (new)
  - `tests/trailers_h2_to_h1.rs` (new)
  - `tests/trailers_h3_to_h1.rs` (new)
  - `tests/trailers_h1_to_h3.rs` (new)

Approach:
**The bridge crate is `lb-l7`.** `BridgeResponse` lives in
`crates/lb-l7/src/lib.rs` (round-2 review confirmed: `~line 48+`).
The bridges that need trailer plumbing are the eight files listed
above; six exist today (per the round-2 review), the remaining two
(`h2_to_h3`, `h3_to_h2`) may already exist as stubs — Round-4 will
confirm and either extend or create them.

**`BridgeResponse` extension (composes with PROTO-2-03):**

```rust
// crates/lb-l7/src/lib.rs
pub struct BridgeResponse<B> {
    pub status: http::StatusCode,
    pub headers: http::HeaderMap,
    pub body: B,                                  // streams data + trailers
    pub trailers: Option<http::HeaderMap>,        // populated when body is finite
    pub informationals: tokio::sync::mpsc::Receiver<Informational>, // PROTO-2-03
}
```

For streaming bodies (the production case), trailers arrive as the
final `Frame::Trailers(_)` on the body stream — `http-body 1.0`'s
canonical shape. The `trailers: Option<HeaderMap>` field is the
*sync-collected* form used by codec tests; the streaming body is
authoritative on the runtime path.

**Strip hop-by-hop from trailers (RFC 9110 §6.6.1):** trailer names
MUST NOT be any of `Connection`, `Keep-Alive`, `Transfer-Encoding`,
`TE`, `Upgrade`, `Trailer` itself, plus the framing headers
`Content-Length`, `Cache-Control`, `Max-Forwards`, `Authorization`,
`Set-Cookie`, `Content-Encoding`, `Content-Type`, `Content-Range`,
`Trailer` (the announce header). The strip happens via a new
`strip_hop_by_hop_trailers` helper in `lb-l7/src/util/hop.rs`,
mirroring the request-side `strip_hop_by_hop`. PROTO-2-07's
`StrippedTrailers` newtype enforces the strip at the type level so
no bridge can forward unstripped trailers.

**Per-bridge implementation:**

  - **H1→H2 / H1→H3**: read H1 chunked trailers via
    `hyper::Response<Body>::body().trailers().await`; emit them as
    a HEADERS frame with END_STREAM (RFC 9113 §8.1, RFC 9114 §4.1).
  - **H2→H1 / H3→H1**: H2/H3 trailers appear as a final HEADERS
    frame with END_STREAM; serialise as chunked-trailer block on
    the H1 socket per RFC 9112 §7.1.2:
    `0\r\n<trailer-name>: <value>\r\n...\r\n\r\n`. Requires the
    H1 response to have advertised `Transfer-Encoding: chunked`
    (the bridge sets this when `trailers` is present).
  - **H2↔H2 / H3↔H3 / H2↔H3**: forward trailer HEADERS frame.

**`Trailer` announce-header handling:** RFC 9110 §6.6.1 *recommends*
that a server announce trailer field names in the `Trailer:`
response header before the body starts. The gateway forwards the
`Trailer:` header verbatim when it arrives from upstream; if
upstream omitted it, the gateway does *not* synthesise one
(synthesising could leak trailer-existence to a client that
wouldn't otherwise know to look). RFC 9110 §6.6.1 makes this
"SHOULD" not "MUST".

**gRPC trailer special-case is preserved.** `grpc_proxy.rs` continues
to synthesise `grpc-status` / `grpc-message` trailers from the
gRPC-Web → gRPC path; this plan does not touch that codepath. The
generic trailer plumbing is the **complement** to that special-case.

Proof:
  - Test: `tests/trailers_h1_to_h2.rs::single_trailer_forwarded`
    Invariant: H1 upstream sends chunked response with
    `Trailer: x-checksum` announce header and a final
    `0\r\nx-checksum: deadbeef\r\n\r\n` block; downstream H2
    client receives a HEADERS frame with `x-checksum: deadbeef`
    and END_STREAM after the body frames.
  - Test: `tests/trailers_h1_to_h2.rs::multiple_trailers_forwarded`
    Invariant: H1 upstream sends three trailer fields; H2 client
    receives all three in the trailer HEADERS frame.
  - Test: `tests/trailers_h2_to_h1.rs::trailers_emitted_as_chunked`
    Invariant: H2 upstream sends a trailer HEADERS frame; H1
    client receives a chunked-trailer block matching RFC 9112
    §7.1.2 byte-exact.
  - Test: `tests/trailers_h3_to_h1.rs::h3_trailers_round_trip`
    Invariant: H3 upstream → H1 downstream; trailer field
    preserved.
  - Test: `tests/trailers_h1_to_h3.rs::h1_chunked_to_h3_trailers`
    Invariant: H1 chunked-trailer → H3 trailer HEADERS frame.
  - Test (hop-by-hop strip): `tests/trailers_h2_to_h1.rs::forbidden_trailer_names_stripped`
    Invariant: upstream sends `transfer-encoding: chunked` as a
    *trailer field*; gateway strips it before forwarding to client.

Risk / blast radius:
  - **Bridge-signature change** affects all eight bridges; this is
    proto's lane per synthesis §D, no cross-team edit conflict.
  - The streaming path adds one branch in each bridge's body-poll
    loop to detect `Frame::Trailers` vs. `Frame::Data`. Negligible
    perf cost.
  - Compatibility: clients that today receive an empty response
    where they expected trailers will now see them. This is the
    intent. No clients lose functionality.
  - sec cross-review §H.1 noted a malicious-upstream smuggle case;
    the hop-by-hop strip on trailers closes it (the upstream
    cannot inject `Transfer-Encoding` via a trailer).

Cross-ref:    composes with PROTO-2-03 (BridgeResponse extension is
              the same Round-4 edit) and PROTO-2-07
              (`StrippedTrailers` newtype); closes PROTO-2-12.
Owner:        proto
Lead-approval: approved 2026-05-13 team-lead
