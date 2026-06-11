# Protocol Model

How ExpressGateway terminates a client protocol and re-originates a (possibly
different) protocol to the backend — the 9-cell matrix, the HTTP/3 stack, gRPC,
and WebSockets. For the operator-facing support table see
[`../features.md`](../features.md); for the bounded constraints see
[`../known-limitations.md`](../known-limitations.md).

## The 9-cell front × back matrix

The front (client-facing) protocol and the back (backend) protocol are chosen
independently, so the gateway proxies a full **3 × 3 = 9-cell** matrix:

| front ↓ \ back → | **H1** | **H2** | **H3** |
|------------------|--------|--------|--------|
| **H1**           | ✅ | ✅ | ✅ |
| **H2**           | ✅ | ✅ | ✅ |
| **H3**           | ✅ | ✅ | ✅ |

- **Front** is the listener `protocol`: `h1`/`h1s` serve HTTP/1.1 (and HTTP/2
  via ALPN on `h1s`); `quic` serves HTTP/3.
- **Back** is the per-backend `protocol`: `tcp`/`h1` → HTTP/1.1, `h2` → HTTP/2,
  `h3` → HTTP/3.

Each cell is exercised by an end-to-end test under `tests/bridging_h{f}_h{b}.rs`
(nine files, `bridging_h1_h1.rs` … `bridging_h3_h3.rs`).

## The protocol-neutral bridge pipeline

The nine cells are not nine independent proxies. They share one pipeline in
`lb-l7`: each front codec decodes a request into a **protocol-neutral**
representation (`StrippedRequest` — `crates/lb-l7/src/stripped_request.rs`), the
bridge re-encodes it for the backend protocol, and the response streams back the
same way. The bridge files are one per cell — `h1_to_h1.rs`, `h1_to_h2.rs`,
`h1_to_h3.rs`, `h2_to_h1.rs`, `h2_to_h2.rs`, `h2_to_h3.rs`, `h3_to_h1.rs`,
`h3_to_h2.rs`, `h3_to_h3.rs` (all in `crates/lb-l7/src/`) — and they wrap the
streaming proxies (`h1_proxy.rs`, `h2_proxy.rs`) and the H3 relay
(`crates/lb-quic/src/h3_bridge.rs`).

Properties that hold across all cells:

- **Hop-by-hop headers are stripped** at the boundary (Connection, Keep-Alive,
  Transfer-Encoding, etc.), per RFC 9110, before re-encoding.
- **Header/trailer translation funnels through typed values** — hyper's
  `HeaderName`/`HeaderValue` for the H1/H2 wire, QPACK for H3 — so a CRLF or NUL
  cannot split a field on egress (response-splitting defense).
- **There is a `MAX_HEADERS` cap** on the header set.
- **Bodies are never whole-buffered.** Requests and responses stream
  frame-by-frame, bounded by 64 MiB caps (a request over the cap gets `413`).
  See [`backpressure.md`](backpressure.md).

## The HTTP/3 stack (quiche::h3)

HTTP/3 termination runs on **quiche 0.29** (BoringSSL), not a hand-rolled H3
layer. `quiche::h3` owns the control + QPACK streams, frame sequencing, and
pseudo-header validation; the gateway's job is the request/response **relay**,
which lives in the per-connection actor (`crates/lb-quic/src/conn_actor.rs`)
driving `crates/lb-quic/src/h3_bridge.rs`. The H3-terminate lifecycle, including
connection recycling, is detailed in [`quic-modes.md`](quic-modes.md). The
migration from the earlier hand-rolled H3 codec to `quiche::h3` is recorded in
[`../decisions/quinn-to-quiche-migration.md`](../decisions/quinn-to-quiche-migration.md);
the now-removed hand-rolled codec survives only as the test-only
`lb-h3-testcodec` crate.

## gRPC

gRPC is proxied **opaquely** over an HTTP/2 or HTTP/3 path. The gateway does not
parse protobuf; it forwards the gRPC frames and — critically — the **trailers**
that carry `grpc-status` / `grpc-message`. The H3-upstream connector propagates
backend response trailers end-to-end to the front (verified). `lb-grpc` supplies
the supporting helpers:

- **Deadline clamp.** A client `grpc-timeout` larger than the configured ceiling
  is clamped before forwarding. The ceiling is `grpc.max_deadline_seconds`,
  **default 300 s** (`crates/lb-config/src/lib.rs`, `default_grpc_max_deadline`).
- **Streaming-mode awareness.** `StreamingMode` distinguishes Unary,
  ServerStreaming, ClientStreaming, and BidiStreaming
  (`crates/lb-grpc/src/streaming.rs`) — all four call shapes are supported.
- **Status translation** between HTTP and gRPC status codes
  (`crates/lb-grpc/src/status.rs`).

> **Known limitation — gRPC needs an H2/H3 front.** gRPC over an **HTTP/1.1
> front does not work**: gRPC's status lives in trailers, and on an HTTP/1.1
> downstream a streamed response cannot deliver trailers (their names aren't
> known at head-time, and the gateway will not re-buffer the whole response to
> learn them). This matches nginx. Terminate gRPC clients on an H2 or H3
> listener. Full rationale:
> [`../known-limitations.md`](../known-limitations.md) ("gRPC requires an
> HTTP/2 or HTTP/3 front").

## WebSockets (H1 / H2 / H3)

WebSocket framing is handled by **tungstenite**; the gateway relays the upgraded
tunnel. Support spans three transports, with different defaults:

| Transport | RFC | Status | Enable |
|-----------|-----|--------|--------|
| **WS over H1** | 6455 (`Upgrade`) | ✅ default-on once `[listeners.websocket]` is present | — |
| **WS over H2** | 8441 (extended CONNECT) | ⛔ **gated OFF by default** | `websocket.h2_extended_connect = true` |
| **WS over H3** | 9220 (extended CONNECT) | ☑️ opt-in | `websocket.h3_extended_connect = true` |

The H1 path lives in `crates/lb-l7/src/ws_proxy.rs`; the H3 tunnel lives in
`crates/lb-quic/src/ws_tunnel.rs`. The config gates (`h2_extended_connect`,
`h3_extended_connect`) both default to `false`
(`crates/lb-config/src/lib.rs`).

> **Why WS-over-H2 is gated off.** The H2 extended-CONNECT tunnel can buffer
> **unbounded** against a stalled peer: hyper's H2 upgrade path sends on the
> stream even when the flow-control window is closed, so the h2 layer buffers
> without backpressure — a DoS surface. This is a hyper limitation (tracked as
> `CF-S27-2`), not a gateway defect, so the feature stays off until it is fixed
> upstream. When the WS block is present but `h2_extended_connect` is unset, an
> H2 extended-CONNECT request is rejected byte-identically to the feature-absent
> case. Full detail:
> [`../known-limitations.md`](../known-limitations.md) ("WebSocket over HTTP/2
> (RFC 8441) is gated OFF by default").

For extended CONNECT (RFC 8441/9220), note the pseudo-header rule the validator
enforces: when `:protocol` is present, `:scheme` and `:path` are **required**
(the opposite of classic CONNECT). An unknown `:protocol` is rejected with
`501`.

## See also

- [`quic-modes.md`](quic-modes.md) — Mode A/B and the H3 connection lifecycle.
- [`backpressure.md`](backpressure.md) — why bodies are never whole-buffered.
- [`../features.md`](../features.md) — the operator support/gating table.
- [`../decisions/ADR-0006-frame-pipeline.md`](../decisions/ADR-0006-frame-pipeline.md)
  and
  [`../decisions/ADR-0002-h2-codec-strategy.md`](../decisions/ADR-0002-h2-codec-strategy.md).
