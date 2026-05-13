# Plan for PROTO-2-03 — 1xx / 100-Continue / 103 Early Hints forwarding policy

Finding-ref:    PROTO-2-03 (medium, Open)
Files touched:
  - `crates/lb-l7/src/lib.rs` (`BridgeResponse` shape — add an
    `informationals` channel)
  - `crates/lb-l7/src/h1_proxy.rs` (forward 1xx on H1 client → upstream
    path; honour `Expect: 100-continue` semantics)
  - `crates/lb-l7/src/h2_proxy.rs` (H2 server: stream 1xx HEADERS frames
    pre-final-response)
  - `crates/lb-l7/src/h2_to_h1.rs`, `h1_to_h2.rs`, `h2_to_h2.rs`,
    `h1_to_h3.rs`, `h3_to_h1.rs`, `h3_to_h3.rs`, `h2_to_h3.rs`,
    `h3_to_h2.rs` (bridges plumb informational frames)
  - `tests/h1_continue_pass_through.rs` (new)
  - `tests/h2_continue_pass_through.rs` (new)
  - `tests/early_hints_pass_through.rs` (new)

Approach:
**Policy decision: pass-through always.** RFC 9110 §15.2 requires a
proxy to forward 1xx responses to the client unless the proxy itself
end-to-end processed the corresponding request. The gateway is a
pure intermediary (no end-to-end termination of application
semantics) so every 1xx upstream response is forwarded verbatim
(with hop-by-hop strip applied to its header set).

Concrete bridge-response shape:

```rust
// crates/lb-l7/src/lib.rs (BridgeResponse extension)
pub struct BridgeResponse<B> {
    pub status: http::StatusCode,
    pub headers: http::HeaderMap,
    pub trailers: Option<http::HeaderMap>, // PROTO-2-12
    pub body: B,
    /// 1xx informational responses received from upstream prior to
    /// the final response. Forwarded verbatim per RFC 9110 §15.2.
    pub informationals: tokio::sync::mpsc::Receiver<Informational>,
}

pub struct Informational {
    pub status: http::StatusCode,      // 100, 102, 103, …
    pub headers: http::HeaderMap,      // with strip_hop_by_hop applied
}
```

For each protocol pair:

  - **H1 → H1**: hyper 1.x's `client::conn::http1::SendRequest`
    surfaces 1xx via the response future; the bridge captures
    them via `client::conn::http1::handshake`'s informational
    callback (hyper 1.x: `Builder::on_informational`). Forward to
    the channel. The downstream H1 server writes the status-line
    +headers verbatim then resumes streaming the body.
  - **H1 → H2/H3**: each 1xx becomes a HEADERS frame with
    `:status = 1xx` and **no END_STREAM** (RFC 9113 §8.1, RFC 9114
    §4.1). hyper's `http2::SendResponse::send_informational`
    handles this.
  - **H2/H3 → H1**: forward as a `HTTP/1.1 1xx Foo\r\n...\r\n\r\n`
    chunk written directly to the downstream socket before the
    final response.
  - **H2 → H2 / H3 → H3 / H2 ↔ H3**: forward HEADERS frame per the
    target binding.

`Expect: 100-continue` semantics: the gateway forwards the
`Expect: 100-continue` header upstream verbatim and holds the
request body until either (a) upstream sends 100 (forwarded to
client; body then streamed) or (b) upstream sends a non-1xx final
response (gateway forwards final response and does **not** send
body upstream — RFC 9110 §10.1.1). A bounded timeout
(`[runtime].expect_continue_timeout_ms`, default 5 s) governs the
hold; on timeout the gateway proceeds as if 100 was received (the
RFC-mandated fallback when no informational arrives).

Slowloris cross-cut (sec cross-review §A.3): while the gateway
holds the request body waiting for upstream 100, the slowloris
read-side timer continues to tick on the *client* connection — a
slow client cannot exploit the hold. SEC-2-03's slowloris detector
operates on the client read deadline independent of this gate.

Proof:
  - Test: `tests/h1_continue_pass_through.rs::client_sees_100_then_response`
    Invariant: upstream returns `HTTP/1.1 100 Continue\r\n\r\n`
    then `HTTP/1.1 200 OK\r\n...`; downstream H1 client receives
    both lines in order.
  - Test: `tests/h1_continue_pass_through.rs::expect_continue_timeout_falls_through`
    Invariant: upstream stalls 6 s on read; gateway sends body
    after 5 s (default timeout); response completes.
  - Test: `tests/h2_continue_pass_through.rs::h2_client_sees_103_then_200`
    Invariant: upstream H1 emits `103 Early Hints\r\nLink: </css>;
    rel=preload\r\n\r\n` then `200 OK`; downstream H2 client
    receives a HEADERS frame with `:status=103` and the `link`
    header, then a separate HEADERS frame with `:status=200`.
  - Test: `tests/early_hints_pass_through.rs::link_header_preserved`
    Invariant: `link` and `priority` headers survive the bridge.
  - Test: `tests/h2_continue_pass_through.rs::informational_no_end_stream`
    Invariant: the H2 HEADERS frame for the 1xx has no END_STREAM
    flag set; only the final 2xx/3xx/4xx/5xx frame carries it.

Risk / blast radius:
  - Bridge-shape change (`BridgeResponse` gains a field). All six
    bridges must update; mechanical edit but spans every bridge
    file in proto's lane.
  - The mpsc channel introduces an allocation per request. Bound
    capacity to 4 (a request rarely sees >2 informationals in
    practice — 100 + 103); on overflow drop oldest with a warn-log
    (informationals are advisory).
  - Interop: clients that today never saw `103 Early Hints` will
    start receiving them. Per RFC 9110 §15.2.3 any client that
    doesn't recognise 103 ignores it. No spec-side risk.

Cross-ref:    closes PROTO-2-03; composes with SEC-2-03 (slowloris
              read-side timer); PROTO-2-12 shares the BridgeResponse
              extension shape (trailers + informationals land in the
              same Round-4 edit pass).
Owner:        proto
Lead-approval: approved 2026-05-13 team-lead
