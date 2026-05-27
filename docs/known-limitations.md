# Known Limitations (operator-facing)

This document lists behaviors operators should be aware of when deploying
ExpressGateway. These are bounded, documented constraints — not defects —
and where relevant they match the behavior of standard reverse proxies.

## gRPC requires an HTTP/2 or HTTP/3 front (not HTTP/1.1)

**Summary:** gRPC works through the gateway only when the **downstream
(client-facing) listener** is HTTP/2 or HTTP/3. An **HTTP/1.1 front cannot
deliver gRPC response trailers** (notably `grpc-status` / `grpc-message`) to
the client, so gRPC over an H1 front is not usable. This matches nginx and
standard reverse-proxy behavior.

**Why:** gRPC carries its status code in HTTP **trailers**. The gateway
streams responses incrementally (bounded memory; it does not buffer whole
response bodies). On an HTTP/1.1 downstream, trailers can only be emitted if
the trailer field names are declared up front in a `Trailer:` header on the
response head — but for a streamed response the trailer values arrive *after*
the body, so their names are not known at head-time, and the gateway will not
re-buffer the entire response just to learn them (that would reintroduce an
unbounded-memory path). hyper's HTTP/1.1 encoder therefore drops trailers on a
streamed response. This is identical to nginx, which likewise does not deliver
trailers added by an upstream on a proxied HTTP/1.1 response.

**What the gateway DOES guarantee:** the H3-upstream connector *propagates*
the backend's response trailers end-to-end up to the front (verified). The
loss is strictly in the HTTP/1.1 downstream encoding step. On an HTTP/2 or
HTTP/3 front the trailers are delivered natively and gRPC works end-to-end.

**Operator guidance:** terminate gRPC clients on an **H2 or H3 listener**. Do
not place gRPC clients behind an HTTP/1.1 front and expect `grpc-status`
trailers to arrive. (Internal reference: CF-RESP-1 / CASE-ii; applies to the
H1→H2 and H1→H3 cells — any H1-downstream streamed response.)

---

_Maintained alongside the H-to-H protocol matrix. As the matrix and protocol
support evolve, new bounded limitations are recorded here for operators._
