# Backpressure & the Bounded-Relay Model (R8)

ExpressGateway never buffers a whole request or response body. The amount of
body data in flight at any instant is bounded by a small, **body-size-independent**
window, and when that window fills the gateway stops reading — which makes the
underlying transport stop extending the peer's flow-control window. Internally
this invariant is called **R8**. This page explains why it exists, the constants
that enforce it per protocol, and the soak evidence that it holds under hostile
load.

## Why no whole-body buffering

A proxy that `collect()`s a body before forwarding it turns "send me a 10 GB
upload" (or a slow trickle) into "allocate 10 GB on the proxy" — a trivial
memory-exhaustion DoS, and a head-of-line stall that defeats streaming. R8 is
the rule that there is **no such path**: bodies stream frame-by-frame, and the
retained memory per stream is a fixed window regardless of total body size.

## Two layers of bound

**1. A total-size cap → `413`.** Requests and responses are capped at **64 MiB**
(`MAX_REQUEST_BODY_BYTES` / `MAX_RESPONSE_BODY_BYTES`,
`crates/lb-quic/src/h3_bridge.rs`; the request cap is re-exported and enforced in
the L7 proxies). A request body that exceeds `MAX_REQUEST_BODY_BYTES` is rejected
with **`413 Payload Too Large`** (RFC 9110 §15.5.14), never an unbounded
allocation (`h1_proxy.rs`, `h2_proxy.rs`). This is the hard ceiling.

**2. A bounded in-flight window, independent of body size.** Within the 64 MiB
ceiling, only a tiny window is ever *retained* at once — a bounded channel of
small chunks. This is the actual backpressure mechanism, and it is the same
shape across protocols:

| Protocol | Where | Depth × chunk | In-flight window |
|----------|-------|---------------|------------------|
| **H3** (front + relay) | `conn_actor.rs` / `h3_bridge.rs` | `H3_BODY_CHANNEL_DEPTH = 8` × `H3_BODY_CHUNK_MAX = 8 KiB` | **~64 KiB** |
| **H2** (request leg) | `h2_proxy.rs` | `H2_REQ_CHANNEL_DEPTH = 8` × `H2_REQ_CHUNK_MAX = 8 KiB` | **~64 KiB** |
| **H1** | `h1_proxy.rs` | streams hyper's `Incoming` by construction (no `collect()`); `H1_REQ_CHUNK_MAX = 8 KiB` | bounded by the stream |
| **Mode B** (raw QUIC) | `raw_proxy.rs` | `STREAM_RELAY_WINDOW = 256 KiB` per stream | **256 KiB / stream** |

Every one of these constants is annotated in source as **independent of the
total body size and of the 64 MiB cap** — that body-independence is the precise
R8 property the memory integration tests assert (they read the gauges behind the
`test-gauges` feature; see [DEV-SETUP](DEV-SETUP.md)).

The H2 window doubles as the validate-before-forward lookahead: a request that
fits inside the 64 KiB window is polled to EOF (driving the same hyper/h2
validation a `collect()` did) *before* the upstream is dialed — so the validation
that whole-buffering used to provide is preserved without the unbounded
allocation (`h2_proxy.rs`, `H2_REQ_CHANNEL_DEPTH`).

## The read-pause mechanism

Backpressure is "stop reading, and the transport propagates the pause":

- **H1 / H2 / H3 body pump.** The inbound body is fed to the upstream through a
  **bounded `mpsc` channel**. When the channel is full, the pump stops calling
  the codec's receive (`stream_recv` for H3) — so quiche/h2 stop extending the
  client's flow-control window, and the client is paused at the transport layer.
  When the upstream drains a chunk, the pump resumes and the window reopens.
- **Mode B raw relay.** Each stream has a fixed `STREAM_RELAY_WINDOW` (256 KiB)
  of pending bytes. Once that window is full the relay stops reading that
  direction, so quiche stops granting the peer more flow-control credit
  (`raw_proxy.rs`).

The net effect is end-to-end: a slow or non-reading downstream peer slows the
upstream read (and vice versa) without the gateway buffering the difference.

> **One trailer subtlety (H1 downstream).** Because the gateway streams rather
> than re-buffers, an HTTP/1.1 **downstream** cannot deliver trailers added by
> the upstream on a streamed response (their names aren't known at head-time).
> This is why gRPC needs an H2/H3 front. See
> [`protocol-model.md`](protocol-model.md#grpc) and
> [`../known-limitations.md`](../known-limitations.md). It is a consequence of
> R8, not a separate buffering bug.

## A smuggling corollary worth knowing

The bounded channel introduced one subtlety the code guards against: a dropped
channel sender makes the receiver's `poll_recv` return `None`, which `StreamBody`
would translate into a **clean** EOF — i.e. hyper would emit the chunked
terminator and the upstream would see a *complete* request. That is the wrong
signal when the inbound stream was reset mid-body (a smuggling vector): a
truncated request must never be relayed as complete. The H2 pump handles this
explicitly (`h2_proxy.rs`, `F-MD-4`). The lesson: bounded streaming has to
distinguish "clean EOF" from "the source died".

## Evidence — R8 holds over hours, not seconds

R8 is not just asserted by unit tests; it is exercised by the **S39 four-hour
burn-in** (`audit/perf/s39-burnin.md`): 12 scenarios run **concurrently** for
14 400 s under hostile load. Result: **11/12 BOUNDED, panic = 0** (the one
analyzer-DRIFT is explained and not a leak). RSS plateaued on every scenario.
Under that load R8 held over **billions of operations**, including:

- the CVE-2023-44487 **H2 rapid-reset** defense bounded over **320 M** resets,
- the **H3 reset flood** bounded over **49 M** floods,
- **261 M** connection-floods bounded,

all with bounded RSS / fd / thread counts and zero panics. The full per-scenario
table is in [`../../audit/perf/s39-burnin.md`](../../audit/perf/s39-burnin.md).

## See also

- [`quic-modes.md`](quic-modes.md) — the `STREAM_RELAY_WINDOW` (Mode B) and the
  H3 body channel in their full context.
- [`protocol-model.md`](protocol-model.md) — the bridge pipeline that all of
  this streams through.
- [`security-and-conformance.md`](security-and-conformance.md) — how the bound
  ties into the DoS posture.
