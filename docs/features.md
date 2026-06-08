# Features & Protocol Support

This document is the operator-facing map of **what ExpressGateway
supports, what is gated off by default, and what is waived**. It reflects
the audited state as of the Session 38 security audit
(`audit/security/s38-findings.md`). For the byte-level schema see
`CONFIG.md`; for the bounded constraints behind each "gated"/"waived" cell
see [`known-limitations.md`](known-limitations.md).

## Protocol matrix (front × back)

The gateway proxies the full **9-cell** matrix: any of HTTP/1.1, HTTP/2,
HTTP/3 on the client-facing (front) side, translated to any of HTTP/1.1,
HTTP/2, HTTP/3 on the backend side. Header/trailer translation funnels
through hyper's typed `HeaderName`/`HeaderValue` (H1/H2 wire) or QPACK
(H3 wire), so CRLF/NUL cannot split a field on egress.

| front ↓ \ back → | **H1** | **H2** | **H3** |
|------------------|--------|--------|--------|
| **H1** (`h1`, `h1s`) | ✅ | ✅ | ✅ |
| **H2** (`h1s` ALPN) | ✅ | ✅ | ✅ |
| **H3** (`quic`)      | ✅ | ✅ | ✅ |

Front protocol is chosen by the **listener** `protocol`: `h1`/`h1s` serve
H1 (and H2 via ALPN on `h1s`); `quic` serves H3. Back protocol is the
per-backend `protocol` (`tcp`/`h1` → HTTP/1.1, `h2` → HTTP/2, `h3` →
HTTP/3). Responses are streamed frame-by-frame; the gateway never buffers
a whole request or response body (bounded by 64 MiB caps + 413/erroring).

## QUIC modes

| Mode | Trigger | Behaviour |
|------|---------|-----------|
| **H3 terminate** (default) | `protocol = "quic"` | Gateway terminates the client QUIC connection and speaks HTTP/3 (`quiche::h3`), then proxies to an H1/H2/H3 backend. |
| **Mode A passthrough** | top-level `[passthrough]` | Routes QUIC flows to backends **by Connection ID without decrypting** — TLS stays end-to-end client↔backend; the gateway holds no TLS state. A parallel datapath (no `[[listeners]]` needed). |
| **Mode B terminate** | `[listeners.quic.raw_proxy]` | Gateway terminates the client QUIC and **re-originates a fresh upstream QUIC** connection, relaying raw streams + datagrams between two distinct `quiche::Connection`s. |

## Application protocols

| Feature | Status | Notes |
|---------|--------|-------|
| **HTTP/1.1, HTTP/2, HTTP/3** | ✅ Supported | H2 served on `h1s` via ALPN (no separate `h2` listener); H3 served on `quic`. |
| **gRPC** | ✅ Supported (H2/H3 front) | Needs an **H2 or H3 front and an `h2`/`h3` backend**. An H1 front cannot deliver `grpc-status` trailers on a streamed response (matches nginx). Deadline clamp + synthesized health check via `[listeners.grpc]`. |
| **WebSocket over H1** | ✅ Supported (default-on) | RFC 6455 `Upgrade`. On by default once `[listeners.websocket]` is present. |
| **WebSocket over H2** | ⛔ **Gated OFF by default** | RFC 8441 extended-CONNECT. Opt-in via `websocket.h2_extended_connect = true`. Off because of a hyper backpressure limitation (CF-S27-2) — an H2 WS stream can buffer unbounded against a stalled peer. See [known-limitations](known-limitations.md). |
| **WebSocket over H3** | ☑️ Opt-in | RFC 9220 extended-CONNECT. Enable via `websocket.h3_extended_connect = true`. |

## TLS

| Aspect | State |
|--------|-------|
| Stack | rustls + BoringSSL (via quiche for QUIC). |
| Versions | TLS 1.2 + 1.3 by default (downgrade-safe: ECDHE-only, AEAD-only; no SSLv3/TLS1.0/1.1, no RC4/CBC-without-EtM). `tls13_only = true` restricts to 1.3 for PCI-DSS-style requirements. |
| Client (mTLS) | **No server-side mTLS** — the gateway does not request a client cert (`with_no_client_auth()`). Intentional for an internet-facing reverse proxy. |
| Upstream verification | **Enforced.** H3 backends verify the upstream cert by default (`tls_verify_peer`, requires `tls_ca_path`); Mode B always verifies. |
| Session tickets | Rotated daily with an overlap window (`TicketRotator`). |

## Conformance

| Suite | Result | Source of truth |
|-------|--------|-----------------|
| **h2spec** (HTTP/2) | **147/147 pass** | `tests/h2spec.rs` + `DEPLOYMENT.md`. |
| **h3spec** (HTTP/3 + QUIC) | Passes with **12 named waivers** | `scripts/ci/h3spec-check.sh` (`CF-QUICHE-UPGRADE`). The waivers are quiche-0.29 transport deviations + QPACK uni-stream items that quiche reads-and-discards (inert, no amplification); a new failure outside the waiver list fails CI. See [known-limitations](known-limitations.md). |

## Operational features

- **SIGHUP config hot reload** with an honesty contract — swappable subset
  (backends, HTTP timeouts) applied live; restart-required changes logged,
  never silently applied; invalid config rolled back atomically. (`CONFIG.md`)
- **SIGUSR1 TLS cert rotation** — atomic `TlsConfigBundle` swap under
  in-flight handshakes.
- **Graceful drain on SIGTERM** — lameduck `/readyz` flip → settle →
  cancel → bounded drain budget (`drain_timeout_ms`, default 10 s).
- **Admin API** — `/metrics` (token-gated), probes `/livez /readyz
  /startupz /healthz`; loopback-only by default. (`README.md`, `METRICS.md`)
- **L4 XDP/eBPF** data plane — single-kernel; bounds-checked packet parse +
  per-CPU new-flow rate cap; validated live on Linux 7.0. (`DEPLOYMENT.md`)

## Security defenses (summary)

The gateway ships a DoS-mitigation catalog enforced on live listeners:
Rapid-Reset (CVE-2023-44487), CONTINUATION flood (CVE-2024-27316), HPACK /
QPACK bomb, SETTINGS / PING flood, zero-window stall, slowloris (header) +
slow-POST (body) timeouts, request smuggling (CL.TE / TE.CL / H2
downgrade), QUIC 0-RTT replay, and the S36 H3 connection-recycling cap.
Full mapping in [`SECURITY.md`](../SECURITY.md).
