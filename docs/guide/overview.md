# What is ExpressGateway?

ExpressGateway is a Rust **L4 + L7 load balancer and reverse proxy**. On the
L7 side it terminates and proxies HTTP/1.1, HTTP/2, and HTTP/3 across the full
**9-cell front × back matrix** (any of {H1, H2, H3} on the client side,
translated to any of {H1, H2, H3} on the backend side), streaming bodies
frame-by-frame with bounded memory. On the L4 side it ships an XDP/eBPF TCP+UDP
data plane and a QUIC-native datapath (route-by-Connection-ID passthrough, or
terminate-and-re-originate). It is one statically-linked binary
(`expressgateway`), configured by a single TOML file, with a file-backed
control plane, Prometheus metrics, and graceful drain on `SIGTERM`.

This page is the orientation map. It states what the gateway is, where it
fits, what it can and cannot do, and where to go next. Every capability below
links to its canonical reference; every limitation links to the bounded reason
in [`known-limitations.md`](../known-limitations.md). Nothing here is
aspirational — if a feature is gated, deferred, or unmeasured, it says so.

## The problem it solves

You have HTTP (or raw TCP, or QUIC) traffic to spread across a backend pool,
and you want to do it at the edge without (a) terminating a body into memory
before forwarding it, (b) crashing on a malformed packet, or (c) being knocked
over by a known protocol-level DoS. ExpressGateway is built around those three
properties:

- **Bounded memory on every cell.** The gateway never buffers a whole request
  or response body; it streams frame-by-frame and enforces 64 MiB caps (and
  returns `413` past them). This holds across all nine front×back combinations.
- **Panic-free libraries.** Every library crate denies `unwrap`/`expect`/
  `panic`/`indexing_slicing` at the crate root and CI's `panic-freedom` job
  enforces it (the binary additionally installs a panic hook that increments
  `panic_total` and logs rather than aborting). See
  [`../../SECURITY.md`](../../SECURITY.md) "Panic-free posture".
- **A DoS-mitigation catalog enforced on live listeners** — Rapid-Reset
  (CVE-2023-44487), CONTINUATION flood (CVE-2024-27316), HPACK/QPACK bomb,
  SETTINGS/PING flood, zero-window stall, slowloris, slow-POST, request
  smuggling (CL.TE / TE.CL / H2-downgrade), QUIC 0-RTT replay. Full mapping in
  [`../../SECURITY.md`](../../SECURITY.md).

## Headline capabilities

| Capability | Summary | Reference |
|------------|---------|-----------|
| **9-cell HTTP matrix** | Front {H1, H2, H3} × back {H1, H2, H3}, streamed, bounded memory. | [`../features.md`](../features.md) |
| **L4 / XDP data plane** | XDP/eBPF TCP+UDP load balancing; compiled BPF ELF ships in-tree, **off by default**, single-kernel, validated live on Linux 7.0. | [`DEPLOYMENT.md`](DEPLOYMENT.md) |
| **QUIC Mode A passthrough** | Route QUIC flows by Connection ID **without decrypting** (TLS stays end-to-end client↔backend). | [`../features.md`](../features.md) |
| **QUIC Mode B terminate** | Terminate the client QUIC and re-originate a fresh upstream QUIC connection, relaying raw streams + datagrams. | [`../features.md`](../features.md) |
| **gRPC** | Native gRPC proxying — requires an **H2 or H3 front** and an `h2`/`h3` backend. | [`../known-limitations.md`](../known-limitations.md) |
| **WebSocket** | Over H1 (RFC 6455, **on by default**); over H2 (RFC 8441, **gated off**); over H3 (RFC 9220, opt-in). | [`../features.md`](../features.md) |
| **TLS** | rustls + BoringSSL; TLS 1.2 + 1.3 by default (downgrade-safe), `tls13_only` opt-in; no server-side mTLS (intentional), upstream verification enforced. | [`../../SECURITY.md`](../../SECURITY.md) |
| **Conformance** | h2spec **147/147**; h3spec passes with **12 named waivers** (`CF-QUICHE-UPGRADE`). | [`../features.md`](../features.md) |
| **Load balancing** | 11 algorithms (round-robin, weighted round-robin, random, weighted random, P2C, Maglev, ring hash, EWMA, least connections, least requests, session affinity) + active/passive health checks. | [`../../README.md`](../../README.md) |
| **Operations** | File-backed control plane; SIGHUP config hot reload (honesty contract); SIGUSR1 cert rotation; graceful drain on SIGTERM; Prometheus `/metrics` + k8s probes. | [`CONFIG.md`](CONFIG.md), [`METRICS.md`](METRICS.md), [`RUNBOOK.md`](RUNBOOK.md) |

A QUIC listener serves HTTP/3; HTTP/2 is served on an `h1s` (HTTPS) listener
via ALPN. There is no separate `"h2"` or `"h3"` listener protocol — see
[`CONFIG.md`](CONFIG.md) "Listener protocols".

## Where it fits — an honest positioning

ExpressGateway is a focused, memory-safe, protocol-complete data-plane proxy.
Its strengths are concrete and verifiable: the full 9-cell HTTP matrix with
bounded streaming, QUIC passthrough and terminate, a hand-audited DoS catalog
(S38 security audit: 0 Critical / 0 High / 1 Medium / 7 Low / 4 Info — see
[`../../SECURITY.md`](../../SECURITY.md)), and panic-free libraries. Wire
parsing is delegated to mature, widely-deployed libraries — hyper (H1/H2),
quiche/BoringSSL (H3/QUIC/TLS), rustls (TLS), tungstenite (WS) — so the
gateway's own attack surface is small and was fuzzed extensively.

It is **not** a drop-in replacement for envoy, traefik, HAProxy, or nginx in
their mature roles. Those projects have years of battle-tested production scale,
broad ecosystems (xDS / dynamic control planes, plugin and filter systems,
WAFs, observability integrations), and operational tooling that a focused
~41-development-session project does not. ExpressGateway has a file-backed
control plane with a trait seam for future backends — not a full xDS-style
dynamic control plane. If you need a large filter/plugin ecosystem, a proven
hyperscale track record, or a managed control plane, the incumbents are
stronger. See [`comparison.md`](comparison.md) for a factual,
capability-by-capability breakdown and a candid "where the incumbents are
stronger" section.

Use ExpressGateway when you want a memory-safe Rust proxy with first-class
HTTP/3 and QUIC, bounded-memory streaming, and an explicit, auditable security
posture, and you are comfortable operating it from a single config file plus
SIGHUP reload.

## Key limitations to know up front

These are the constraints most likely to affect an evaluation. Each is bounded
and documented; the canonical "why" lives in
[`known-limitations.md`](../known-limitations.md) and the supported/gated/waived
matrix in [`features.md`](../features.md). The consolidated view with
"who-does-this-affect" framing is [`capabilities.md`](capabilities.md).

- **gRPC requires an HTTP/2 or HTTP/3 front.** An HTTP/1.1 front cannot deliver
  `grpc-status` trailers on a streamed response (this matches nginx). Terminate
  gRPC clients on an `h1s` (H2-via-ALPN) or `quic` (H3) listener.
- **WebSocket over HTTP/2 (RFC 8441) is gated OFF by default.** A hyper H2
  upgrade-path limitation (CF-S27-2) can buffer unbounded against a stalled
  peer, so the feature ships off; run WS over H1 (default) or H3. Opt in only
  behind a trusted client tier.
- **The L4 XDP data plane is single-kernel and off by default.** The shipped
  BPF ELF is validated against a specific kernel window (5.15 / 6.1 / 6.6 LTS,
  plus live on 7.0); it is not built CO-RE-portable in this drive. With XDP
  off (the default), L4 traffic goes through the kernel stack normally.
- **No server-side mTLS** (intentional for an internet-facing proxy); upstream
  certificate verification **is** enforced.
- **Deferred performance tiers.** io_uring and XDP-offload throughput are **not
  yet characterized**. The measured numbers come from one co-located 8-core box
  (see [`PERFORMANCE.md`](PERFORMANCE.md)) — read the conditions before relying
  on any figure.

## Where to next

| If you want to… | Go to |
|-----------------|-------|
| Build it and serve your first request | [`getting-started.md`](getting-started.md) |
| See the full supported / gated / waived view | [`capabilities.md`](capabilities.md) + [`../features.md`](../features.md) |
| Understand the bounded constraints in detail | [`../known-limitations.md`](../known-limitations.md) |
| Compare it to envoy / traefik / HAProxy / nginx | [`comparison.md`](comparison.md) |
| Read the measured performance baseline | [`PERFORMANCE.md`](PERFORMANCE.md) |
| Configure every knob | [`CONFIG.md`](CONFIG.md) |
| Deploy to production (systemd, capabilities, XDP) | [`DEPLOYMENT.md`](DEPLOYMENT.md) |
| Operate it (alerts, drain, triage) | [`RUNBOOK.md`](RUNBOOK.md) |
| Scrape metrics | [`METRICS.md`](METRICS.md) |
| Review the security model | [`../../SECURITY.md`](../../SECURITY.md) |
| Read the architecture / crate graph | [`../architecture.md`](../architecture.md), [`../arch/`](../arch/) |
