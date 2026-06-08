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

## WebSocket over HTTP/2 (RFC 8441) is gated OFF by default

**Summary:** WS-over-HTTP/2 (extended-CONNECT, RFC 8441) is **disabled by
default**. It is opt-in via `websocket.h2_extended_connect = true`.
WS-over-H1 (RFC 6455) and WS-over-H3 (RFC 9220) are unaffected.

**Why:** the H2 extended-CONNECT tunnel can buffer unbounded against a
stalled peer. hyper's H2 upgrade path sends on the stream even when the
flow-control window is closed, so the h2 layer buffers without
backpressure — a DoS surface against a slow/non-reading WS peer. This is a
hyper limitation (tracked as **CF-S27-2**), not a gateway defect; until it
is addressed upstream the feature stays off so an operator does not enable
an unbounded-write path unknowingly. When the block is present but
`h2_extended_connect` is unset, an H2 extended-CONNECT request is rejected
byte-identically to the feature-absent case.

**Operator guidance:** run WebSocket over **H1** (default) or **H3**. Only
enable `h2_extended_connect` behind a trusted, well-behaved client tier.

---

## No server-side mTLS

**Summary:** the gateway does **not** request a client certificate
(`with_no_client_auth()`); there is no server-side mutual-TLS enforcement.

**Why:** this is the normal posture for an internet-facing reverse proxy —
clients are browsers/anonymous. It is recorded so the deployment profile is
explicit, not because it is an oversight. **Upstream (backend) certificate
verification IS enforced** (H3 backends verify by default; Mode B always
verifies). TLS 1.2 is allowed by default but is downgrade-safe (ECDHE +
AEAD only); set `tls13_only = true` for TLS-1.3-only environments.

**Operator guidance:** terminate client-auth requirements at a layer that
supports it, or treat the trust boundary as "any internet client" (which is
the gateway's threat model — see `SECURITY.md`).

---

## Mode A passthrough relies on the QUIC Retry round-trip

**Summary:** Mode A (`[passthrough]`) admits a new QUIC flow only after a
valid **stateless Retry token** bound to the peer address
(`mint_retry = true`, the default). This is the Initial-flood / source-spoof
defense.

**Why / limitation:** the QUIC connection-table cap is a global budget
(`max_quic_connections`, default 100 000) with **no per-source-IP
sub-cap** — a single real (Retry-completing) IP can consume the whole
budget. Off-path spoofed-source attackers cannot fill the table (they must
complete the Retry from a real address), so this is a fairness/tunability
gap, not an unbounded vector (audit F-RES-3, LOW, hardening carry-forward).

**Operator guidance:** keep `mint_retry = true` and
`strict_source_binding`/`flow_idle_timeout_ms` at defaults unless you
understand the trade-off (see the foot-gun table in `CONFIG.md`).

---

## h2spec / h3spec named waivers

**Summary:** **h2spec passes 147/147.** **h3spec passes with 12 named
waivers** (`CF-QUICHE-UPGRADE`, enumerated in `scripts/ci/h3spec-check.sh`).

**Why:** the 12 h3spec waivers are quiche-0.29 transport-layer deviations
(e.g. the first-packet `CONNECTION_CLOSE` suppression class) plus QPACK
encoder/decoder uni-stream instruction items that quiche **reads and
discards** — they are inert (no dynamic table is ever allocated, so no
amplification) and the gateway has no hook to change quiche's behaviour.
The waiver list is exact: a *new* h3spec failure outside the named set
fails CI, so the waiver cannot silently grow.

**Operator guidance:** none required — these are upstream library
deviations with no security impact, documented for transparency.

---

## L4 XDP data plane is single-kernel

**Summary:** the compiled BPF ELF ships in-tree
(`crates/lb-l4-xdp/src/lb_xdp.bin`) and the loader attaches it; it is
validated against a specific kernel/verifier window (5.15 / 6.1 / 6.6 LTS,
plus live-validated on 7.0). It is **not** built CO-RE-portable across
arbitrary kernels in this drive.

**Why:** multi-kernel verifier portability (BTF/CO-RE across the full
matrix) is carried as a separate item (F-ESC-1). On AWS ENA, native
(`xdpdrv`) attach additionally requires `MTU ≤ 3498` and reduced channel
count (the shipped object is built without XDP multi-buffer/frags). See
`DEPLOYMENT.md` "ENA native-XDP requirements".

**Operator guidance:** deploy on a validated LTS kernel, or run with XDP
disabled (`[runtime].xdp_enabled = false`, the default) — L4 traffic then
goes through the kernel TCP/UDP stack normally with no loss.

---

_Maintained alongside the H-to-H protocol matrix. As the matrix and protocol
support evolve, new bounded limitations are recorded here for operators._
