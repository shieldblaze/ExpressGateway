# Known Limitations (operator-facing)

This is the single, canonical list of behaviors to be aware of when
deploying ExpressGateway. Each entry is a bounded, documented
constraint — not a defect — and where relevant it matches the behavior of
standard reverse proxies. Every item below answers the same three
questions: **what** the limit is, **who it affects**, and how to
**work around** it. Unfamiliar term? See [`glossary.md`](glossary.md).

The supported/gated/waived matrix lives in [`features.md`](features.md);
the threat model and residual-risk register live in
[`../SECURITY.md`](../SECURITY.md).

---

## gRPC requires an HTTP/2 or HTTP/3 front (not HTTP/1.1)

**What:** gRPC works through the gateway only when the **client-facing
listener** is HTTP/2 or HTTP/3. An **HTTP/1.1 front cannot deliver gRPC
response trailers** (notably `grpc-status` / `grpc-message`) to the
client, so gRPC over an H1 front is not usable. This matches nginx and
standard reverse-proxy behavior.

**Why:** gRPC carries its status code in HTTP **trailers**. The gateway
streams responses incrementally and never buffers a whole response body.
On an HTTP/1.1 downstream, trailers can only be emitted if their field
names are declared up front in a `Trailer:` header — but for a streamed
response the trailer values arrive *after* the body, so their names are
not known at head-time, and the gateway will not re-buffer the entire
response just to learn them (that would reintroduce an unbounded-memory
path). hyper's HTTP/1.1 encoder therefore drops trailers on a streamed
response, exactly as nginx does. The gateway *does* propagate the
backend's trailers end-to-end up to the front; the loss is strictly in
the HTTP/1.1 downstream encoding step. On an HTTP/2 or HTTP/3 front the
trailers are delivered natively and gRPC works end-to-end.

**Who it affects:** anyone terminating gRPC clients. Not affected: plain
HTTP, REST, or anything that does not rely on trailers.

**Mitigation:** terminate gRPC clients on an `h1s` (H2-via-ALPN) or
`quic` (H3) listener, with an `h2`/`h3` backend. Do not place gRPC
clients behind an HTTP/1.1 front and expect `grpc-status` to arrive.
(Applies to the H1→H2 and H1→H3 cells — any H1-downstream streamed
response.)

---

## Load-balancing algorithm selection is not config-exposed

**What:** the live data path uses **round-robin** (L7 HTTP and raw-TCP
listeners) and **Maglev consistent-hashing by Connection ID** (QUIC Mode
A passthrough). Ten further algorithms are implemented in the
`lb-balancer` library but are **not selectable via configuration** —
there is no policy key, and the config schema rejects unknown keys. The
per-backend `weight` field is accepted but not enforced.

**Who it affects:** anyone planning to choose a specific policy — sticky
sessions / session-affinity, least-connections, P2C, ring-hash, EWMA, or
weighted distribution. Not affected: deployments that are fine with
round-robin (and Maglev-by-Connection-ID for Mode A passthrough).

**Mitigation:** design around round-robin in this build; if you need
session stickiness, key it at a layer in front of the gateway. The full
behavior (and the library's status) is documented at
[`features.md`](features.md) "Load balancing".

---

## Passive health tracking is not wired into backend selection

**What:** ExpressGateway tracks passive per-backend health (a
consecutive-success/failure state machine), but in this build it is **not
consulted by the balancer** and is **not fed by live traffic** — it is
seeded at startup only. **Active probing** (interval/path/expected-status)
is **deferred (REL-2-05)** and not implemented. The balancer will keep
sending traffic to a backend that is failing.

**Who it affects:** anyone relying on the gateway to automatically eject
an unhealthy backend from rotation. Not affected: deployments where an
external system (orchestrator, L4 load balancer, service mesh) already
performs health-based ejection.

**Mitigation:** put health-based ejection in front of, or behind, the
gateway (e.g. orchestrator readiness gating, or a service registry that
only publishes healthy backends). Details at [`features.md`](features.md)
"Load balancing".

---

## WebSocket over HTTP/2 (RFC 8441) is gated OFF by default

**What:** WS-over-HTTP/2 (extended-CONNECT, RFC 8441) is **disabled by
default**; opt-in via `websocket.h2_extended_connect = true`. WS-over-H1
(RFC 6455) and WS-over-H3 (RFC 9220) are unaffected. When the block is
present but the flag is unset, an H2 extended-CONNECT request is rejected
byte-identically to the feature-absent case.

**Why:** the H2 extended-CONNECT tunnel can buffer unbounded against a
stalled peer. The underlying hyper H2 upgrade path sends on the stream
even when the flow-control window is closed, so the layer below buffers
without backpressure — a DoS surface against a slow/non-reading WS peer.
This is a library limitation, not a gateway defect; until it is addressed
upstream the feature stays off so an operator does not unknowingly enable
an unbounded-write path.

**Who it affects:** anyone wanting WebSocket specifically over HTTP/2. Not
affected: HTTP traffic, or WebSocket over H1/H3.

**Mitigation:** run WebSocket over **H1** (default-on) or **H3**
(opt-in) — both unaffected. Enable `h2_extended_connect` only behind a
trusted, well-behaved client tier.

---

## No server-side mTLS

**What:** the gateway does **not** request a client certificate
(`with_no_client_auth()`); there is no server-side mutual-TLS
enforcement. **Upstream (backend) certificate verification IS enforced**
(H3 backends verify by default; Mode B always verifies).

**Why:** this is the normal posture for an internet-facing reverse proxy —
clients are browsers/anonymous. It is recorded so the deployment profile
is explicit, not because it is an oversight. TLS 1.2 is allowed by default
but is downgrade-safe (ECDHE + AEAD only); set `tls13_only = true` for
TLS-1.3-only environments.

**Who it affects:** anyone needing client-certificate authentication at
the gateway. Not affected: the standard internet-facing posture ("any
internet client").

**Mitigation:** terminate client-auth requirements at a layer that
supports it, or treat the trust boundary as "any internet client" (the
gateway's threat model — see [`../SECURITY.md`](../SECURITY.md)).

---

## Mode A passthrough relies on the QUIC Retry round-trip

**What:** Mode A (`[passthrough]`) admits a new QUIC flow only after a
valid **stateless Retry token** bound to the peer address
(`mint_retry = true`, the default) — the Initial-flood / source-spoof
defense. The QUIC connection-table cap is a **global** budget
(`max_quic_connections`, default 100 000) with **no per-source-IP
sub-cap**: a single real (Retry-completing) IP can consume the whole
budget.

**Why:** off-path spoofed-source attackers cannot fill the table — they
must complete the Retry from a real address — so this is a
fairness/tunability gap, not an unbounded vector (a low-severity
hardening carry-forward; see the residual-risk register in
[`../SECURITY.md`](../SECURITY.md)).

**Who it affects:** Mode A passthrough deployments concerned about a
single abusive but real client monopolizing the flow table. Not affected:
H3-terminate, Mode B, or any non-passthrough deployment.

**Mitigation:** keep `mint_retry = true` and the
`strict_source_binding` / `flow_idle_timeout_ms` knobs at defaults unless
you understand the trade-off (see the foot-gun table in
[`guide/CONFIG.md`](guide/CONFIG.md)).

---

## h2spec / h3spec named waivers

**What:** **h2spec passes 147/147.** **h3spec passes with 12 named
waivers** (`CF-QUICHE-UPGRADE`, enumerated in
`scripts/ci/h3spec-check.sh`).

**Why:** the 12 h3spec waivers are quiche-0.29 transport-layer deviations
(e.g. the first-packet `CONNECTION_CLOSE` suppression class) plus QPACK
encoder/decoder uni-stream instruction items that quiche **reads and
discards** — they are inert (no dynamic table is ever allocated, so no
amplification) and the gateway has no hook to change quiche's behaviour.
The waiver list is exact: a *new* h3spec failure outside the named set
fails CI, so the waiver list cannot silently grow.

**Who it affects:** anyone requiring a clean h3spec sweep for compliance —
review the named list. Not affected: functional or security posture (no
security impact).

**Mitigation:** none required — these are upstream library deviations,
documented for transparency.

---

## L4 XDP data plane is single-kernel and off by default

**What:** the compiled BPF ELF ships in-tree
(`crates/lb-l4-xdp/src/lb_xdp.bin`) and the loader attaches it, but only
when explicitly enabled (`[runtime].xdp_enabled = true`; the default is
`false`). The object is validated against a specific kernel/verifier
window (5.15 / 6.1 / 6.6 LTS, plus live-validated on 7.0). It is **not**
built CO-RE-portable across arbitrary kernels in this drive.

**Why:** multi-kernel verifier portability (BTF/CO-RE across the full
matrix) is carried as a separate portability item. On AWS ENA, native
(`xdpdrv`) attach additionally requires `MTU ≤ 3498` and a reduced channel
count (the shipped object is built without XDP multi-buffer/frags). See
[`guide/DEPLOYMENT.md`](guide/DEPLOYMENT.md) "ENA native-XDP
requirements".

**Who it affects:** anyone enabling the XDP data plane on an unvalidated
kernel or with jumbo-frame MTU on ENA. Not affected: the default
configuration (XDP off).

**Mitigation:** deploy on a validated LTS kernel, or leave XDP disabled
(the default) — L4 traffic then goes through the kernel TCP/UDP stack
normally with no loss.

---

## No binary hot-restart via socket-descriptor handover

**What:** the binary sets `SO_REUSEPORT` on listening sockets so a
supervisor can run a replacement process **side-by-side** during a manual
upgrade, but it does **not** transfer listening sockets between processes.
An in-process binary-upgrade handover is not implemented.

**Why:** socket-descriptor handover between processes is deferred; the
supported upgrade path is side-by-side processes plus graceful drain.

**Who it affects:** anyone expecting Envoy/HAProxy/Pingora-style
hot-restart through listening-socket inheritance. Not affected:
deployments that roll instances behind an L4 load balancer, or that
accept a brief side-by-side overlap.

**Mitigation:** start the replacement process side-by-side under
`SO_REUSEPORT`, then graceful-drain the old one with `SIGTERM` (lameduck →
settle → bounded drain). See [`../README.md`](../README.md) and
[`guide/RUNBOOK.md`](guide/RUNBOOK.md). Topology and multi-instance
patterns are covered in
[`guide/deployment-patterns.md`](guide/deployment-patterns.md).

---

## QUIC H3-terminate and Mode B are single-backend (not load-balanced)

**What:** a `quic` **H3-terminate** listener forwards to the **first**
`[[listeners.backends]]` entry only — additional entries are ignored, and the
pool is not round-robined. **Mode B** (`[listeners.quic.raw_proxy]`)
re-originates to a single `backend_addr` by schema. Neither spreads load
across multiple upstreams. (QUIC **Mode A passthrough** is the exception: it
Maglev-hashes flows across its `[passthrough].backends` pool.)

**Why:** H3-terminate and Mode B each manage one upstream QUIC/HTTP
connection per client connection; multi-backend selection on these paths is
not wired in this build. The live round-robin picker runs on the L7 HTTP and
raw-TCP paths.

**Who it affects:** anyone expecting an H3 (`quic`) listener, or a Mode B
proxy, to balance across a backend pool. Not affected: L7 HTTP / raw-TCP
listeners (round-robin across the pool) and Mode A passthrough
(Maglev-by-Connection-ID).

**Mitigation:** put multiple H3-terminate / Mode B instances behind an L4/L3
load balancer to spread load (see
[`guide/deployment-patterns.md`](guide/deployment-patterns.md)), or use Mode A
passthrough, where the gateway itself spreads QUIC flows across the pool. The
schema detail is in [`guide/CONFIG.md`](guide/CONFIG.md)
"`[[listeners.backends]]`" and the policy detail in
[`features.md`](features.md) "Load balancing".

---

## Graceful drain does not emit proactive per-protocol close signals

**What:** on `SIGTERM` the gateway drains by flipping `/readyz` to 503
(lameduck), settling for the upstream-probe window, then waiting a bounded
budget for in-flight requests to finish before force-closing any survivors.
It does **not** yet emit proactive per-protocol drain signals — HTTP/1.1
`Connection: close`, HTTP/2 `GOAWAY`, or HTTP/3 `CONNECTION_CLOSE` — to tell
live clients to stop reusing the connection. A long-lived connection still
open when the drain budget elapses is **force-closed (aborted), not
gracefully signalled**.

**Why:** the per-protocol emission code exists but is not yet plumbed into the
per-connection serve loops; it is a tracked follow-up. Until it lands, the
drain relies on the lameduck → settle → bounded-wait sequence, which is
connection-safe for short-request workloads (they finish well inside the
budget) but cannot proactively wind down a long-lived stream.

**Who it affects:** deployments with long-lived connections (gRPC bidi,
WebSocket, SSE, long-poll) that want a *graceful* per-connection wind-down on
deploy. Not affected: short-request HTTP, which completes inside the drain
budget.

**Mitigation:** size the drain budget to your workload — raise the
per-listener `drain_timeout_ms` for streaming listeners so in-flight work
finishes before the deadline, and keep the fronting LB's lameduck /
endpoint-removal aligned with `readiness_settle_ms` so new traffic stops
before the drain starts. The drain sequence, the per-protocol drain-signal
matrix, and the budget-tuning guidance are in
[`guide/RUNBOOK.md`](guide/RUNBOOK.md) "Drain (graceful shutdown)" and "Tuning
the drain budget".

---

_Maintained alongside the front×back protocol matrix in
[`features.md`](features.md). As the matrix and protocol support evolve,
new bounded limitations are recorded here for operators._
