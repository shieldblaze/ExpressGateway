# How ExpressGateway Compares

This page positions ExpressGateway against the mature incumbents — **Envoy**,
**Traefik**, **HAProxy**, and **nginx** — factually and with honest tradeoffs.
It is written for an engineer evaluating proxies. It makes **no "faster than X"
or "better than X" claims**; it states verifiable capabilities and is candid
about where the incumbents are stronger.

For ExpressGateway's own claims, every cell links back to its source elsewhere
in these docs. For **Envoy, HAProxy, nginx, and Katran** the comparison points
draw on the in-repo reference-system research under
[`../research/`](../research/) ([`envoy.md`](../research/envoy.md),
[`haproxy.md`](../research/haproxy.md), [`nginx.md`](../research/nginx.md),
[`katran.md`](../research/katran.md)); **Traefik** and any fast-moving release
details are summarized from each project's own current documentation, not an
in-repo study. Capabilities of fast-moving projects change between releases —
verify against the incumbent's current documentation for a procurement
decision.

## Capability / protocol comparison

| Capability | ExpressGateway | Envoy | Traefik | HAProxy | nginx |
|------------|----------------|-------|---------|---------|-------|
| **HTTP/1.1** | ✅ | ✅ | ✅ | ✅ | ✅ |
| **HTTP/2** | ✅ | ✅ | ✅ | ✅ | ✅ |
| **HTTP/3 (QUIC) terminate** | ✅ (`quiche`) | ✅ (`quiche`) | ☑️ experimental | ✅ (homegrown QUIC) | ✅ (`quiche`-based) |
| **QUIC passthrough (by Connection ID, no decrypt)** | ✅ Mode A | ⚠️ not a first-class mode | ⚠️ not a first-class mode | ⚠️ TCP/UDP forward, not QUIC-CID-aware | ⚠️ stream-level UDP proxy, not QUIC-CID-aware |
| **gRPC proxy** | ✅ (H2/H3 front) | ✅ | ✅ | ✅ | ✅ |
| **WebSocket** | ✅ H1; ⛔ H2 (gated); ☑️ H3 | ✅ incl. H2 (RFC 8441) | ✅ | ✅ | ✅ |
| **Response compression** (gzip/brotli) | ❌ (pass-through only) | ✅ | ✅ | ✅ | ✅ |
| **L4 / TCP load balancing** | ✅ | ✅ | ✅ | ✅ | ✅ (stream module) |
| **L4 via XDP/eBPF (kernel data plane)** | ✅ (off by default, single-kernel) | ❌ (userspace) | ❌ | ❌ | ❌ |
| **Language / memory safety** | Rust (panic-free libs, `unsafe` minimized) | C++ | Go | C | C |
| **TLS** | rustls + BoringSSL; 1.2+1.3; no server mTLS | BoringSSL; incl. mTLS | Go crypto/tls; incl. mTLS | OpenSSL/variants; incl. mTLS | OpenSSL; incl. mTLS |
| **Dynamic control plane** | file-backed + trait seam (no xDS) | ✅ xDS (reference impl) | ✅ providers (k8s, Docker, …) | runtime API / DPAPI | ⚠️ commercial (NGINX Plus) / SIGHUP |
| **Config reload without dropping connections** | ✅ SIGHUP (swappable subset) | ✅ xDS hot update | ✅ dynamic | ✅ `-sf` / `SO_REUSEPORT` | ✅ SIGHUP |
| **Binary upgrade via FD handover** | ❌ (deferred; `SO_REUSEPORT` side-by-side only) | ✅ hot restart | n/a | ✅ `-sf` | ✅ |
| **Plugin / filter ecosystem** | ❌ (hand-written bridges, no user filters) | ✅ large (HTTP filters, Wasm, Lua) | ✅ middleware/plugins | ✅ Lua, SPOE | ✅ modules, OpenResty/Lua |

Legend: ✅ supported · ☑️ opt-in / experimental · ⚠️ partial or different shape ·
❌ not provided. Cells marked ⚠️ for QUIC passthrough mean the incumbent can
forward UDP/TCP but does not route QUIC by Connection ID without decrypting — a
shape ExpressGateway provides as a first-class Mode A datapath
([`../features.md`](../features.md)).

Notes on the ExpressGateway column:

- **HTTP/3 + QUIC** via quiche — h3spec passes with 12 named waivers
  ([`capabilities.md`](capabilities.md)).
- **WebSocket over H2 is gated off by default** (a hyper backpressure
  limitation); Envoy supports RFC 8441 WS-over-H2. Run WS over H1/H3
  ([`../known-limitations.md`](../known-limitations.md)).
- **gRPC requires an H2/H3 front** (an H1 front cannot deliver `grpc-status`
  trailers — matches nginx) ([`../known-limitations.md`](../known-limitations.md)).
- **XDP/eBPF L4** ships in-tree but is **off by default** and single-kernel
  ([`DEPLOYMENT.md`](DEPLOYMENT.md)). The userspace incumbents do not run an
  XDP data plane (Meta's Katran is the canonical XDP L4 LB — see
  [`../research/katran.md`](../research/katran.md)).
- **No server-side mTLS** (intentional); the incumbents offer it. Upstream
  verification is enforced ([`../../SECURITY.md`](../../SECURITY.md)).
- **No response compression.** ExpressGateway passes content through unmodified;
  it does not gzip/brotli responses. The incumbents all offer response
  compression, and evaluators routinely look for it.

## Where ExpressGateway fits

ExpressGateway is a **focused, memory-safe data-plane proxy** with first-class
HTTP/3 and QUIC. It is a strong fit when you want:

- **The full 9-cell HTTP matrix with bounded-memory streaming** — any
  front-protocol to any back-protocol, never buffering a whole body
  ([`../features.md`](../features.md)).
- **First-class QUIC**: both H3 terminate and the unusual **Mode A passthrough**
  (route by Connection ID without decrypting, TLS end-to-end), plus Mode B
  re-origination.
- **A memory-safe Rust implementation** with panic-free libraries enforced in
  CI, and wire parsing delegated to mature libraries (hyper, quiche/BoringSSL,
  rustls, tungstenite) so the hand-rolled surface is small and was fuzzed
  ([`../../SECURITY.md`](../../SECURITY.md)).
- **An explicit, auditable security posture** — the security audit (0 Critical /
  0 High / 1 Medium fixed / 7 Low / 4 Info) and a documented DoS-mitigation
  catalog.
- **Single-binary, single-config-file operation** with SIGHUP hot reload
  (restart-required changes are logged, never silently applied).
- **An optional in-kernel XDP/eBPF L4 data plane** for environments that can run
  it on a validated kernel.

## Where the mature incumbents are stronger

ExpressGateway is a relatively young project (~41 development sessions). It does
**not** match the incumbents on the dimensions that take years and a large user
base to build, and an evaluator should weigh these:

- **Battle-tested production scale.** Envoy, HAProxy (in production since 2001),
  and nginx have run at enormous scale across a vast, adversarial user base for
  years; their bug histories and CVE responses are a public, hard-won corpus
  ([`../research/haproxy.md`](../research/haproxy.md),
  [`../research/nginx.md`](../research/nginx.md)). ExpressGateway's track record
  is short by comparison and its published performance baseline is from a single
  co-located test box ([`PERFORMANCE.md`](PERFORMANCE.md)), not large-scale
  production.
- **Control-plane maturity.** Envoy is the **reference implementation of xDS**
  (CDS/EDS/LDS/RDS/SDS) and integrates with Istio, Consul, and other control
  planes ([`../research/envoy.md`](../research/envoy.md)); Traefik has a broad
  provider model (Kubernetes, Docker, …). ExpressGateway has a **file-backed
  control plane with a trait seam** for future backends — no xDS, no dynamic
  service-discovery integrations out of the box.
- **Ecosystem breadth.** Envoy's HTTP-filter / Wasm / Lua ecosystem, nginx's
  module and OpenResty/Lua ecosystem, HAProxy's Lua/SPOE, and Traefik's
  middleware/plugins provide extensibility ExpressGateway does not — it uses
  hand-written per-protocol bridges with **no user-extensible filter chain**
  ([`../research/envoy.md`](../research/envoy.md) "Mapping to ExpressGateway").
- **Operational features.** Hot binary restart via FD handover (Envoy, HAProxy
  `-sf`, Pingora), unified overload managers, circuit breakers, outlier
  detection / panic mode, retry budgets, and rich access logging are mature in
  the incumbents; ExpressGateway has per-attack mitigations but not these
  higher-level constructs, and its health tracking is **passive and not yet
  wired into backend selection** (active probing is deferred) — see
  [`../known-limitations.md`](../known-limitations.md). The incumbents'
  active/passive health checking is a genuine gap here
  ([`../research/envoy.md`](../research/envoy.md) lists several constructs as gaps).
- **WAF / policy / auth integrations and managed offerings.** The incumbents
  have established WAF integrations, auth filters, and (for some) commercial
  support tiers. ExpressGateway is the open data-plane binary.

## Quick picker

- **Pick ExpressGateway** if HTTP/3 + QUIC (including no-decrypt passthrough),
  bounded-memory streaming, and a memory-safe, auditable data plane are your
  priorities, and a single config file plus SIGHUP reload fits your operating
  model.
- **Pick Envoy or Traefik** if you need a dynamic / xDS control plane, service
  discovery, or a user-extensible filter / plugin / WASM ecosystem.
- **Pick HAProxy or nginx** if you need a decade-plus hyperscale track record,
  response compression, server-side mTLS, or hot binary restart available today.

## Bottom line

Pick ExpressGateway for a memory-safe, protocol-complete Rust proxy with
first-class HTTP/3 + QUIC (including no-decrypt passthrough), bounded-memory
streaming, and an explicit security posture, operated from a single config file.
Pick a mature incumbent when you need a proven hyperscale track record, a
dynamic/xDS control plane, a large filter/plugin ecosystem, server-side mTLS,
hot binary restart, or established WAF/auth integrations. For the
constraint-by-constraint detail behind ExpressGateway's column, see
[`capabilities.md`](capabilities.md) and
[`../known-limitations.md`](../known-limitations.md).
