# Glossary

Definitions for the jargon used across the ExpressGateway docs. Each entry
links the page that explains the concept in depth. Terms are alphabetical
(numerics first).

---

**9-cell matrix** — the 3×3 set of front→back combinations the gateway
proxies: any of {HTTP/1.1, HTTP/2, HTTP/3} on the client side translated to
any of {HTTP/1.1, HTTP/2, HTTP/3} on the backend side. → [`features.md`](features.md)
"Protocol matrix" · [`arch/protocol-model.md`](arch/protocol-model.md).

**ALPN** — Application-Layer Protocol Negotiation, the TLS handshake
extension that selects the protocol (e.g. `h2`). The gateway serves HTTP/2
on an `h1s` listener via ALPN — there is no separate `h2` listener.
→ [`features.md`](features.md).

**Backpressure** — when a slow reader on one side makes the gateway stop
reading from the other side instead of buffering, so the slowness
propagates back to the sender and memory stays bounded.
→ [`arch/backpressure.md`](arch/backpressure.md).

**Bounded-memory streaming** — relaying request/response bodies
frame-by-frame without ever buffering a whole body, so memory stays bounded
under large or slow transfers (64 MiB caps; `413` on exceed).
→ [`arch/backpressure.md`](arch/backpressure.md).

**Bridge / StrippedRequest** — `StrippedRequest` is the protocol-neutral
internal request (typed header names/values, no wire framing) that each of
the nine front→back **bridges** (`h{1,2,3}_to_h{1,2,3}`) translates
through. Funnelling every cell through it is why CRLF/NUL cannot split a
field on egress. → [`arch/protocol-model.md`](arch/protocol-model.md).

**CID** — see **Connection ID**.

**Connection ID (CID)** — the identifier QUIC carries in each packet so an
endpoint can recognise a connection even when the client's address changes.
Mode A passthrough routes by the CID **without decrypting**.
→ [`arch/quic-modes.md`](arch/quic-modes.md).

**conntrack** — the flow-tracking table in the L4/XDP datapath that maps an
established flow to its chosen backend so later packets of the same flow
follow the same path. → [`features.md`](features.md) "L4 XDP" ·
[`guide/DEPLOYMENT.md`](guide/DEPLOYMENT.md).

**CO-RE** — "Compile Once – Run Everywhere," a BPF technique that uses
kernel BTF type information so one compiled object loads across kernel
versions. The shipped XDP object is **not** built CO-RE-portable
(single-kernel). → [`known-limitations.md`](known-limitations.md).

**Drain / settle** — the graceful-shutdown phases on `SIGTERM`: flip to
lameduck (report not-ready), **settle** (let in-flight requests finish),
then cancel/close within a bounded budget (`drain_timeout_ms`).
→ [`features.md`](features.md) · [`guide/RUNBOOK.md`](guide/RUNBOOK.md).

**eBPF** — see **XDP / eBPF**.

**EWMA** — exponentially-weighted moving-average latency routing, a
backend-selection algorithm. Implemented in the library but **not
config-selectable**, and its latency input is fed only in tests.
→ [`features.md`](features.md) "Load balancing".

**extended CONNECT (RFC 8441 / 9220)** — a `CONNECT` request that carries a
`:protocol` pseudo-header, used to tunnel WebSocket over HTTP/2 (RFC 8441)
or HTTP/3 (RFC 9220). → [`features.md`](features.md) ·
[`known-limitations.md`](known-limitations.md).

**Front / back protocol** — *front* is the client-facing listener protocol
(H1/H2/H3); *back* is the per-backend protocol (`tcp`/`h1`/`h2`/`h3`). The
pair picks one of the nine matrix cells. → [`features.md`](features.md).

**GOAWAY** — the HTTP/2 and HTTP/3 control frame a server sends to stop
accepting **new** streams while finishing existing ones; used in graceful
drain and the HTTP/3 connection-recycling cap. → [`features.md`](features.md).

**h2spec / h3spec** — external conformance test suites for HTTP/2 (h2spec)
and HTTP/3 + QUIC (h3spec). → [`features.md`](features.md) "Conformance".

**HPACK** — see **QPACK / HPACK**.

**lameduck** — a drain state in which the gateway reports not-ready
(`/readyz` flips) but keeps serving in-flight requests, so an upstream load
balancer stops sending new traffic before shutdown.
→ [`features.md`](features.md) · [`guide/RUNBOOK.md`](guide/RUNBOOK.md).

**Maglev** — Google's consistent-hashing algorithm (minimal reshuffling
when the backend set changes). Used over the Connection ID for Mode A
passthrough. → [`features.md`](features.md) "Load balancing" ·
[`arch/quic-modes.md`](arch/quic-modes.md).

**Mode A / Mode B** — the two raw-QUIC datapaths. **Mode A** (passthrough)
routes flows by Connection ID without decrypting (TLS stays end-to-end
client↔backend). **Mode B** (terminate) ends the client QUIC and
re-originates a fresh upstream QUIC, relaying raw streams + datagrams. The
default `quic` behaviour, **H3-terminate**, is distinct from both.
→ [`arch/quic-modes.md`](arch/quic-modes.md) · [`features.md`](features.md)
"QUIC modes".

**P2C** — power-of-two-choices: pick two backends at random and choose the
less-loaded one. A backend-selection algorithm; implemented but not
config-selectable. → [`features.md`](features.md) "Load balancing".

**panic-free** — the production library crates are compiled with lints that
forbid panicking constructs (`unwrap`/`expect`/`panic`/array-indexing/…),
enforced in CI. → [`arch/overview.md`](arch/overview.md) ·
[`../SECURITY.md`](../SECURITY.md).

**passthrough vs terminate** — *passthrough* forwards encrypted bytes
without decrypting (Mode A); *terminate* ends the client TLS/QUIC
connection and proxies decrypted requests (H3-terminate and Mode B).
→ [`arch/quic-modes.md`](arch/quic-modes.md).

**QPACK / HPACK** — the header-compression formats for HTTP/3 (QPACK) and
HTTP/2 (HPACK). The gateway translates headers through typed values, so a
compressed header cannot smuggle a field on egress.
→ [`arch/protocol-model.md`](arch/protocol-model.md).

**Retry (QUIC)** — a stateless round-trip in which the server returns a
signed token the client must echo, proving it owns its source address
before a connection is admitted (the Initial-flood / source-spoof defense).
Mode A passthrough requires it (`mint_retry = true`).
→ [`known-limitations.md`](known-limitations.md) ·
[`guide/CONFIG.md`](guide/CONFIG.md).

**ring-hash** — consistent hashing over a hash ring with virtual nodes, a
backend-selection algorithm. Implemented but not config-selectable.
→ [`features.md`](features.md) "Load balancing".

**session-affinity (sticky sessions)** — hash-based routing that keeps a
client landing on the same backend. Implemented in the library but **not
config-selectable** in this build. → [`features.md`](features.md) "Load
balancing".

**SO_REUSEPORT** — a socket option that lets multiple processes bind the
same port, so a supervisor can run a replacement process side-by-side. The
gateway sets it but does **not** itself transfer sockets between processes.
→ [`guide/deployment-patterns.md`](guide/deployment-patterns.md) ·
[`known-limitations.md`](known-limitations.md).

**StrippedRequest** — see **Bridge / StrippedRequest**.

**terminate** — see **passthrough vs terminate**.

**XDP / eBPF** — XDP (eXpress Data Path) is a kernel hook that processes
packets at the driver layer before the normal network stack; eBPF is the
in-kernel sandboxed VM such programs run in. ExpressGateway's optional L4
data plane is an XDP/eBPF program (off by default).
→ [`features.md`](features.md) "L4 XDP" ·
[`guide/DEPLOYMENT.md`](guide/DEPLOYMENT.md).
</content>
