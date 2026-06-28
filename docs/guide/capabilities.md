# Capabilities & Limitations

A one-screen, legend-coded map of **what ExpressGateway supports, what is gated
off, what is waived, and what is deferred**. Every row points at the page that
owns the detail — this page is the index, not the explanation.

- The bounded **why** behind every gated/waived/deferred item lives in
  [`../known-limitations.md`](../known-limitations.md) (with a "who does this
  affect?" note on each).
- The protocol **matrix** and feature detail live in
  [`../features.md`](../features.md).
- Every config **knob** and its reload class live in [`CONFIG.md`](CONFIG.md).
- The threat model and audit posture live in [`../../SECURITY.md`](../../SECURITY.md).

Legend: ✅ supported · ⛔ gated off by default · ☑️ opt-in · ⚠️ bounded
limitation · ⏳ deferred / not yet characterized.

## HTTP protocol matrix (front × back)

The full **9-cell** matrix — any of {H1, H2, H3} on the client side translated to
any of {H1, H2, H3} on the backend — streamed frame-by-frame with bounded memory
(64 MiB caps + `413`). Front protocol is the **listener** `protocol`; back
protocol is the **per-backend** `protocol`. H2 is served on an `h1s` listener via
ALPN; H3 on a `quic` listener. Detail: [`../features.md`](../features.md).

| front ↓ \ back → | H1 | H2 | H3 |
|------------------|----|----|----|
| **H1** | ✅ | ✅ | ✅ |
| **H2** | ✅ | ✅ | ✅ |
| **H3** | ✅ | ✅ | ✅ |

## QUIC modes

| Mode | Status | Detail |
|------|--------|--------|
| **H3 terminate** (default for `quic`) | ✅ | [`../features.md`](../features.md) |
| **Mode A passthrough** (`[passthrough]`) — route by Connection ID, no decrypt | ✅ ⚠️ | [`../known-limitations.md`](../known-limitations.md) |
| **Mode B terminate** (`[listeners.quic.raw_proxy]`) — re-originate upstream QUIC | ✅ | [`../features.md`](../features.md) |

## Application protocols

| Feature | Status | Detail |
|---------|--------|--------|
| HTTP/1.1, HTTP/2, HTTP/3 | ✅ | [`../features.md`](../features.md) |
| **gRPC** (needs an H2/H3 front + `h2`/`h3` backend) | ✅ ⚠️ | [`../known-limitations.md`](../known-limitations.md) |
| **WebSocket over H1** (RFC 6455) | ✅ | [`../features.md`](../features.md) |
| **WebSocket over H2** (RFC 8441) | ⛔ | [`../known-limitations.md`](../known-limitations.md) |
| **WebSocket over H3** (RFC 9220) | ☑️ | [`../features.md`](../features.md) |
| **Response compression** | ❌ | pass-through only (incumbents have it) — [comparison.md](comparison.md) |

## Load balancing & health

| Aspect | Status | Detail |
|--------|--------|--------|
| **Round-robin** (L7 HTTP + raw-TCP) | ✅ | live selection policy in this build — [`../features.md`](../features.md) "Load balancing" |
| **Maglev by Connection ID** (QUIC Mode A) | ✅ | passthrough flow routing — [`../features.md`](../features.md) |
| Other algorithms (weighted RR, P2C, ring-hash, EWMA, least-conn, least-request, random, weighted-random, session-affinity, Maglev-for-L7) | ⏳ | implemented in `lb-balancer` but **not yet selectable from config** — [`../features.md`](../features.md) |
| Per-backend `weight` | ⏳ | parsed but **not enforced** in this build (round-robin ignores it) — [`CONFIG.md`](CONFIG.md) |
| **Passive** health tracking | ⏳ | implemented but **not yet wired into selection** — [`../known-limitations.md`](../known-limitations.md) |
| **Active** health probing (interval/path/expected-status) | ⏳ | deferred — [`../known-limitations.md`](../known-limitations.md) |

## TLS

| Aspect | Status | Detail |
|--------|--------|--------|
| Stack (rustls + BoringSSL) | ✅ | [`../features.md`](../features.md) |
| TLS 1.2 + 1.3 (downgrade-safe); `tls13_only` opt-in | ✅ ☑️ | [`../features.md`](../features.md) |
| Server-side mTLS | ⚠️ not provided (intentional) | [`../known-limitations.md`](../known-limitations.md) |
| Upstream certificate verification | ✅ enforced | [`../../SECURITY.md`](../../SECURITY.md) |

## Conformance

| Suite | Status | Detail |
|-------|--------|--------|
| **h2spec** (HTTP/2) | ✅ 147/147 | [`../features.md`](../features.md) |
| **h3spec** (HTTP/3 + QUIC) | ⚠️ passes with 12 named waivers | [`../known-limitations.md`](../known-limitations.md) |

## L4 / XDP data plane

| Aspect | Status | Detail |
|--------|--------|--------|
| XDP/eBPF TCP+UDP fast path | ✅ ⚠️ off by default (`[runtime].xdp_enabled`) | [`DEPLOYMENT.md`](DEPLOYMENT.md) |
| Kernel portability | ⚠️ single-kernel (not CO-RE-portable) | [`../known-limitations.md`](../known-limitations.md) |
| ENA native (`xdpdrv`) attach | ⚠️ MTU ≤ 3498 + reduced channels | [`DEPLOYMENT.md`](DEPLOYMENT.md) |

With XDP off (the default), L4 traffic uses the kernel TCP/UDP stack normally
with no loss.

## Operations

| Feature | Status | Detail |
|---------|--------|--------|
| SIGHUP config hot reload (swappable subset) | ✅ | [`CONFIG.md`](CONFIG.md) |
| SIGUSR1 cert rotation · SIGTERM graceful drain | ✅ | [`RUNBOOK.md`](RUNBOOK.md) |
| Binary hot-restart via socket-descriptor handover | ⚠️ deferred (`SO_REUSEPORT` side-by-side only) | [deployment-patterns.md](deployment-patterns.md) |
| Per-IP + in-flight connection caps | ✅ | [`CONFIG.md`](CONFIG.md) |
| Topology / HA (stateless; no built-in clustering) | ⚠️ | [deployment-patterns.md](deployment-patterns.md) |
| Prometheus `/metrics` + k8s probes; `LB_LOG_FORMAT` | ✅ | [`METRICS.md`](METRICS.md) |

## Security posture

The security audit closed at **0 Critical / 0 High / 1 Medium (fixed) / 7 Low / 4
Info**. Production wire parsing is delegated to hyper (H1/H2), quiche/BoringSSL
(H3/QUIC/TLS), rustls (TLS), and tungstenite (WS); the only hand-rolled
production parser is the Mode A QUIC public-header reader, which is panic-free by
construction and was fuzzed extensively. Full record:
[`../../SECURITY.md`](../../SECURITY.md).
