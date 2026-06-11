# Capabilities & Limitations

This is the consolidated, at-a-glance view of **what ExpressGateway supports,
what is gated off by default, what is waived, and what is deferred** — with a
"who does this affect?" note on every constraint so you can decide whether a gap
touches your use case.

This page **summarizes and links**; it does not restate the reasoning. The
canonical sources are:

- [`../features.md`](../features.md) — the supported / gated / waived **matrix**.
- [`../known-limitations.md`](../known-limitations.md) — the bounded **why**
  behind each gated/waived item.
- [`../../SECURITY.md`](../../SECURITY.md) — the threat model, the S38 audit
  posture, and residual risks.
- [`CONFIG.md`](CONFIG.md) — every config knob and the reload classification.

Legend: ✅ supported · ⛔ gated off by default · ☑️ opt-in · ⚠️ bounded
limitation · ⏳ deferred / not yet characterized.

## HTTP protocol matrix (front × back)

The gateway proxies the full **9-cell** matrix — any of {H1, H2, H3} on the
client-facing side translated to any of {H1, H2, H3} on the backend — streamed
frame-by-frame with bounded memory (no whole-body buffering; 64 MiB caps +
`413`).

| front ↓ \ back → | H1 | H2 | H3 |
|------------------|----|----|----|
| **H1** | ✅ | ✅ | ✅ |
| **H2** | ✅ | ✅ | ✅ |
| **H3** | ✅ | ✅ | ✅ |

Front protocol is set by the **listener** `protocol`; back protocol is the
**per-backend** `protocol`. HTTP/2 is served on an `h1s` (HTTPS) listener via
ALPN; HTTP/3 on a `quic` listener. There is no separate `"h2"`/`"h3"` listener
protocol. See [`../features.md`](../features.md) "Protocol matrix" and
[`CONFIG.md`](CONFIG.md) "Listener protocols".

## QUIC modes

| Mode | Status | What it does |
|------|--------|--------------|
| **H3 terminate** (default for `quic`) | ✅ | Terminate client QUIC, speak HTTP/3 (`quiche::h3`), proxy to an H1/H2/H3 backend. |
| **Mode A passthrough** (`[passthrough]`) | ✅ ⚠️ | Route QUIC flows by Connection ID **without decrypting** — TLS stays end-to-end client↔backend. See the fairness note below. |
| **Mode B terminate** (`[listeners.quic.raw_proxy]`) | ✅ | Terminate client QUIC and re-originate a fresh upstream QUIC connection, relaying raw streams + datagrams. |

## Application protocols

| Feature | Status | Notes |
|---------|--------|-------|
| HTTP/1.1, HTTP/2, HTTP/3 | ✅ | H2 via ALPN on `h1s`; H3 on `quic`. |
| **gRPC** | ✅ ⚠️ | Requires an **H2 or H3 front** and an `h2`/`h3` backend (see below). |
| **WebSocket over H1** (RFC 6455) | ✅ | On by default once `[listeners.websocket]` is present. |
| **WebSocket over H2** (RFC 8441) | ⛔ | Gated OFF by default (`h2_extended_connect`). See below. |
| **WebSocket over H3** (RFC 9220) | ☑️ | Opt-in via `websocket.h3_extended_connect = true`. |

## TLS

| Aspect | Status |
|--------|--------|
| Stack | ✅ rustls + BoringSSL (BoringSSL via quiche for QUIC). |
| Versions | ✅ TLS 1.2 + 1.3 by default, **downgrade-safe** (ECDHE + AEAD only); `tls13_only = true` opt-in. |
| Server-side mTLS | ⚠️ **Not provided** (intentional for an internet-facing proxy). |
| Upstream verification | ✅ **Enforced** (H3 backends verify by default; Mode B always verifies). |

## Conformance

| Suite | Result | Source of truth |
|-------|--------|-----------------|
| **h2spec** (HTTP/2) | ✅ **147/147 pass** | `tests/h2spec.rs`, [`DEPLOYMENT.md`](DEPLOYMENT.md) |
| **h3spec** (HTTP/3 + QUIC) | ⚠️ passes with **12 named waivers** | `scripts/ci/h3spec-check.sh` (`CF-QUICHE-UPGRADE`) |

## L4 / XDP data plane

| Aspect | Status | Notes |
|--------|--------|-------|
| XDP/eBPF TCP+UDP LB | ✅ ⚠️ | Compiled BPF ELF ships in-tree; loader attaches it. **Off by default** (`[runtime].xdp_enabled = false`). |
| Kernel portability | ⚠️ single-kernel | Validated 5.15 / 6.1 / 6.6 LTS + live on 7.0; **not** CO-RE-portable in this drive (F-ESC-1). |
| ENA native (`xdpdrv`) attach | ⚠️ | Requires `MTU ≤ 3498` + reduced channels — see [`DEPLOYMENT.md`](DEPLOYMENT.md) "ENA native-XDP requirements". |

With XDP off (the default), L4 traffic goes through the kernel TCP/UDP stack
normally with no loss.

---

## Gated / waived / deferred — with who-it-affects

### gRPC requires an H2 or H3 front ⚠️

**What:** gRPC works only when the **client-facing listener** is H2 or H3. An
HTTP/1.1 front cannot deliver `grpc-status` / `grpc-message` **trailers** on a
streamed response (the gateway will not re-buffer a whole body to learn trailer
names — that would reintroduce an unbounded-memory path). The H1→H2 and H1→H3
cells lose trailers on an H1 downstream. This matches nginx.

**Who it affects:** anyone terminating gRPC clients. **Mitigation:** terminate
gRPC on an `h1s` (H2-via-ALPN) or `quic` (H3) listener; the backend must be
`h2`/`h3`. On an H2/H3 front, gRPC works end-to-end (trailers are delivered
natively). Not affected: plain HTTP, REST, anything that doesn't rely on
trailers.

**Canonical why:** [`../known-limitations.md`](../known-limitations.md) "gRPC
requires an HTTP/2 or HTTP/3 front" · matrix: [`../features.md`](../features.md).

### WebSocket over H2 (RFC 8441) gated OFF ⛔

**What:** WS-over-H2 extended-CONNECT is **disabled by default**; opt in via
`websocket.h2_extended_connect = true`. The H2 extended-CONNECT tunnel can
buffer unbounded against a stalled peer (a hyper H2 upgrade-path limitation,
tracked as **CF-S27-2** — a library limitation, not a gateway defect). When the
block is present but the flag is unset, an H2 extended-CONNECT request is
rejected byte-identically to the feature-absent case.

**Who it affects:** anyone wanting WebSocket specifically over HTTP/2.
**Mitigation:** run WebSocket over **H1** (default-on, RFC 6455) or **H3**
(opt-in, RFC 9220) — both unaffected. Enable `h2_extended_connect` only behind a
trusted, well-behaved client tier. Not affected: HTTP traffic, or WS over H1/H3.

**Canonical why:** [`../known-limitations.md`](../known-limitations.md)
"WebSocket over HTTP/2 (RFC 8441) is gated OFF by default".

### WebSocket over H3 (RFC 9220) opt-in ☑️

**What:** WS-over-H3 is off until `websocket.h3_extended_connect = true`.

**Who it affects:** anyone wanting WebSocket over HTTP/3 (it works; it just
isn't on by default). **Canonical:** [`../features.md`](../features.md) ·
[`CONFIG.md`](CONFIG.md) "`[listeners.websocket]`".

### Mode A passthrough — global QUIC connection cap, no per-IP sub-cap ⚠️

**What:** Mode A admits a new QUIC flow only after a valid stateless **Retry
token** bound to the peer address (`mint_retry = true`, default — the
Initial-flood / source-spoof defense). The connection-table cap
(`max_quic_connections`, default 100 000) is a **global** budget with **no
per-source-IP sub-cap**: a single real (Retry-completing) IP can consume the
whole budget. Off-path spoofed sources cannot — they must complete the Retry
from a real address. This is a fairness/tunability gap, **not** an unbounded
vector (audit F-RES-3, LOW).

**Who it affects:** Mode A passthrough deployments concerned about a single
abusive but real client monopolizing the flow table. **Mitigation:** keep
`mint_retry = true` and the source-binding / idle-timeout knobs at defaults
unless you understand the trade-off. Not affected: H3-terminate, Mode B, or any
non-passthrough deployment.

**Canonical why:** [`../known-limitations.md`](../known-limitations.md) "Mode A
passthrough relies on the QUIC Retry round-trip" · residual risk in
[`../../SECURITY.md`](../../SECURITY.md) (F-RES-3).

### h3spec — 12 named waivers ⚠️

**What:** h3spec passes with **12 named waivers** (`CF-QUICHE-UPGRADE`,
enumerated in `scripts/ci/h3spec-check.sh`): quiche-0.29 transport-layer
deviations plus QPACK uni-stream instruction items that quiche **reads and
discards** (inert — no dynamic table is allocated, so no amplification). The
waiver list is exact: a *new* failure outside the named set fails CI, so it
cannot silently grow. **No security impact.**

**Who it affects:** anyone requiring a clean h3spec sweep for compliance —
review the named list. **Mitigation:** none required; documented for
transparency. **Canonical why:** [`../known-limitations.md`](../known-limitations.md)
"h2spec / h3spec named waivers".

### No server-side mTLS ⚠️

**What:** the gateway does not request a client certificate
(`with_no_client_auth()`). Normal posture for an internet-facing reverse proxy.
**Upstream** (backend) cert verification **is** enforced.

**Who it affects:** anyone needing client-certificate authentication at the
gateway. **Mitigation:** terminate client-auth at a layer that supports it, or
treat the trust boundary as "any internet client" (the gateway's threat model).
**Canonical why:** [`../known-limitations.md`](../known-limitations.md) "No
server-side mTLS" · [`../../SECURITY.md`](../../SECURITY.md) (F-INFRA-03).

### L4 XDP is single-kernel and off by default ⚠️

**What:** the shipped BPF ELF is validated against a specific kernel/verifier
window (5.15 / 6.1 / 6.6 LTS + live on 7.0); it is **not** built CO-RE-portable
across arbitrary kernels in this drive (F-ESC-1). On AWS ENA, native (`xdpdrv`)
attach additionally requires `MTU ≤ 3498` and a reduced channel count.

**Who it affects:** anyone enabling the XDP data plane on an unvalidated kernel
or with jumbo-frame MTU on ENA. **Mitigation:** deploy on a validated LTS
kernel, or run with XDP disabled (the default) — L4 then uses the kernel
TCP/UDP stack with no loss. **Canonical why:**
[`../known-limitations.md`](../known-limitations.md) "L4 XDP data plane is
single-kernel" · [`DEPLOYMENT.md`](DEPLOYMENT.md).

### Deferred performance tiers ⏳

**What:** **io_uring** and **XDP-offload** throughput are **not yet
characterized**. The published baseline ([`PERFORMANCE.md`](PERFORMANCE.md))
was measured on one **co-located 8-core box** (load client + gateway + backend
share the cores), so absolute throughput is a shared-box system number, not a
dedicated-rig number.

**Who it affects:** anyone sizing capacity from the published numbers — read the
box spec and the co-located caveat first; do not extrapolate to a dedicated rig.
**Canonical:** [`PERFORMANCE.md`](PERFORMANCE.md) "What was not characterized".

### No binary hot-restart via socket-descriptor handover ⚠️

**What:** the binary sets `SO_REUSEPORT` on listening sockets so a supervisor
can run a replacement process **side-by-side** during a manual handover, but it
does **not** transfer listening sockets between processes (deferred). An
in-process binary-upgrade handover is not implemented.

**Who it affects:** anyone expecting envoy/HAProxy/Pingora-style hot-restart via
listener-socket inheritance. **Mitigation:** run the replacement process
side-by-side under `SO_REUSEPORT` and graceful-drain (`SIGTERM`) the old one.
**Canonical:** [`../../README.md`](../../README.md) (graceful drain) ·
[`RUNBOOK.md`](RUNBOOK.md).

---

## Security posture (summary)

The S38 security audit verdict was **0 Critical / 0 High / 1 Medium (fixed) / 7
Low / 4 Info**. Production wire parsing is delegated to hyper (H1/H2),
quiche/BoringSSL (H3/QUIC/TLS), rustls (TLS), and tungstenite (WS); the only
hand-rolled production parser is `lb_quic::public_header` (Mode A), which is
panic-free by construction and was fuzzed extensively (0 crashes). Full record:
[`../../SECURITY.md`](../../SECURITY.md).
