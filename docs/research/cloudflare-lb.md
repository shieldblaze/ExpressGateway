# Cloudflare's Global Load Balancing (Unimog, Edge LB, Magic Transit)

## Why we study it
Cloudflare runs one of the largest L4+L7 load-balancing fleets on the planet — every datacenter acts as both an origin-facing and client-facing LB, over anycast, at multi-terabit scale. The stack they have published across many blog posts — Unimog (L4), the edge HTTP proxy (first NGINX, now Pingora), Magic Transit (L3 customer traffic), and the Spectrum TCP/UDP proxy — is the most-documented real-world LB topology we can study. For ExpressGateway's horizon (single-node first, multi-node later), their decisions on consistent hashing, session affinity across anycast, DDoS absorption, and control-plane propagation are directly applicable.

## Architecture in brief
Cloudflare's edge is anycast: a single IP is announced from hundreds of POPs, and BGP + routing gets a client to "the nearest POP that's up." Inside a POP, traffic fans out in layers.

**Layer 3: Magic Transit.** Customer traffic arrives over BGP into Cloudflare's edge. The Magic Transit service protects customer IP prefixes with DDoS scrubbing and routes clean traffic over GRE / IPsec tunnels back to the customer's origin network. The data plane uses a mix of XDP programs and upstream routing hardware.

**Layer 4: Unimog.** Every packet entering a Cloudflare server first hits Unimog, their XDP-based L4 LB, which decides which local host should process the packet. Unimog replaces a prior IPVS/LVS deployment. It uses consistent hashing (a variant of Maglev) over the destination service + 5-tuple to pick a target host; encapsulates in a GUE (Generic UDP Encapsulation) wrapper; returns via DSR. The target host decapsulates and hands the packet up the socket stack. Unimog is documented in "Unimog, Cloudflare's edge load balancer" (2020) and related posts.

**Layer 4: Spectrum.** For non-HTTP TCP/UDP (gaming, SSH, MQTT, custom protocols), Spectrum runs a userspace proxy that terminates the TCP/TLS and forwards. Uses the same anycast frontend but a different origin-selection path.

**Layer 7: the edge HTTP proxy.** Until 2022 this was NGINX with extensive Lua modification; since then it's been Pingora (see `docs/research/pingora.md`). The L7 proxy does TLS termination, caching, WAF, rate limiting, and upstream selection (to customer origin or to Workers or to other Cloudflare services).

**Cross-POP load balancing.** Cloudflare sells a "Load Balancing" product to customers which health-checks their origins globally and steers traffic via DNS or via anycast with geo-steering. This is logically distinct from the internal LB stack but shares ideas.

**Control plane.** Configuration is pushed from a central quicksilver KV store (multi-POP replicated) to every edge server. Pingora and Unimog subscribe to change streams and apply atomically. Quicksilver is documented in "How we built Quicksilver" (2020).

**Concurrency model.** Unimog is XDP, so per-RX-queue lock-free. Pingora is Tokio, per-core workers. Spectrum is also Tokio-based. Inter-process coordination via Quicksilver for config, Prometheus-equivalent for stats, and Kafka-equivalent for logs.

## Key design patterns
1. **Anycast + consistent hashing inside the POP.** Anycast gets traffic to a POP; consistent hashing inside picks a host. Both layers preserve session stickiness under membership changes.
2. **GUE encapsulation with DSR.** Unimog encapsulates forwarded packets so the target host sees the original 5-tuple; responses bypass the LB entirely.
3. **One config store, many subscribers.** Quicksilver is the single source of truth; every subsystem subscribes to the slice it cares about. Avoids per-subsystem bespoke distribution.
4. **XDP for L4, userspace for L7.** Kernel fast-path for "forward this packet" decisions; userspace where policy (TLS, cache, WAF) requires it. Same split we're making.
5. **Graceful upgrades with health drain.** Edge servers are drained via BGP AS-path prepending before software upgrade; no cold-start traffic lands during rollout.
6. **Per-customer rate limiting via distributed counters.** Counters replicate across POPs with bounded-staleness; exact counting only for billing, approximate for enforcement.
7. **TLS fingerprinting (JA3/JA4) as a first-class anti-abuse input** — fed into bot-detection and rate-limiting decisions.
8. **Caching as a hierarchical fleet.** Tiered cache: edge → regional → origin. Each tier does its own LB among backends.
9. **Connection coalescing for HTTP/2 / HTTP/3.** If a client opens multiple origins that resolve to Cloudflare, the edge may coalesce onto one H2 connection per origin cert — massive reduction in handshake volume.
10. **DDoS detection in the data plane.** XDP programs sample traffic and emit summaries; an ML-driven control plane tunes per-POP filters in near-real-time. See "Gatebot" and "dosd" posts.

## Edge cases / hard-won lessons
- **POP-level membership changes rehash at L4.** Unimog's hot-swap is critical: adding/removing hosts must not break in-flight TCP connections. A conntrack table papers over the rehash, same idea as Katran.
- **Anycast flap under BGP churn.** If a client's route flips to a new POP mid-TCP-session, the packet lands on a host that has no conntrack entry; session dies. Cloudflare accepts this as rare and relies on client retry.
- **Cache poisoning through host-header smuggling.** A cached response keyed only on URL can be served across hosts; cache key must include the authority. Long public CVE history.
- **Pingora migration from NGINX revealed HTTP/2 behavioural differences** that broke specific customers (header casing, trailer handling). Lesson: conformance tests before cutover, not after.
- **Quicksilver staleness window.** Config pushes are bounded-staleness, not synchronous. A purge might not reach all POPs for a few seconds — documented SLA.
- **Certificate issuance bottlenecks** during major launches (Keyless SSL, Universal SSL). Batch issuance pipeline rate-limited CA.
- **Zone-level vs account-level config merging.** Customer-facing config has inheritance; edge proxies must resolve the effective config deterministically or customers see non-deterministic behaviour.
- **Workers cold-start when not warm on a POP.** A Workers request landing on a POP with no cached isolate adds startup latency; fixed with cross-POP preemptive warming.
- **Kernel TCP retransmit interactions with XDP_TX.** Early Unimog had rare cases where retransmits hashed differently due to a stale conntrack; fixed by making conntrack lookup authoritative over Maglev.
- **H3 / QUIC path migration** over anycast. When a client's NAT rebinds, the new 4-tuple may hash to a different host; Cloudflare uses QUIC connection IDs to steer continuity.
- **TLS session tickets vs anycast.** A ticket issued at POP A may land at POP B on resumption; tickets are encrypted with a globally synchronised key so any POP can decrypt.
- **IPv6 extension header abuse.** Attackers craft packets with many IPv6 extension headers to defeat XDP parsers; Unimog caps parse depth and drops over-budget packets.

## Mapping to ExpressGateway
| Cloudflare pattern | Our equivalent | Gap / status |
| --- | --- | --- |
| L4 XDP Maglev | `crates/lb-l4-xdp/ebpf/src/main.rs` + `crates/lb-l4-xdp/src/lib.rs` (simulation) | Same algorithm. DSR is out of scope. |
| L7 userspace proxy | `crates/lb-l7/` + `crates/lb-h{1,2,3}/` + `crates/lb-quic/` | Present. |
| Centralised config store | `crates/lb-controlplane/src/lib.rs` (`ConfigBackend` trait) | Trait-based; file-backed in-process rather than cross-node KV. Quicksilver analogue is not in scope. |
| Health-drain for upgrade | `crates/lb-health/src/lib.rs` + `crates/lb-controlplane/src/lib.rs` | Health primitives present; BGP-level drain is out of scope. |
| Graceful config rollout | `crates/lb-controlplane/src/lib.rs` | SIGHUP + ArcSwap + rollback; bounded-staleness distributed push is future work. |
| TLS fingerprinting | (none) | Not in scope for v1. |
| Cache tiering | (none) | Not in scope. |
| Connection coalescing | `crates/lb-h2/src/lib.rs` | H2 multiplexing primitives present; explicit origin-coalescing policy is not. |
| Session stickiness across rehash | `crates/lb-l4-xdp/src/lib.rs` (conntrack) + `crates/lb-balancer/src/session_affinity.rs` | Present at both L4 sim and L7 affinity levels. |
| QPACK/QUIC connection ID steering | `crates/lb-quic/src/lib.rs`, `crates/lb-h3/src/lib.rs` | QUIC/H3 crates present; connection-ID-based backend steering is a policy we could add. |
| DDoS sampling + filter tuning | `crates/lb-security/src/lib.rs` | Per-attack mitigations present; sampling/ML-driven tuning is out of scope. |

## Adoption recommendations
- We should design `lb-controlplane` around bounded-staleness push semantics even while the backend is a local file — so a future swap to a distributed backend (etcd, Consul, Quicksilver-like) doesn't require a data-plane rewrite.
- We could add a `ConnectionId` hash input to our L4 balancer so that QUIC path migration keeps flows on the same backend (the Cloudflare H3 lesson).
- We should document a host-header-aware cache key policy even before we ship caching — making the header a first-class input, not an afterthought.
- We could add TLS-fingerprint input (JA4) as an opaque hash in `lb-core` for future WAF/balancer policies, without shipping the full fingerprinting library.
- We should keep L4 and L7 decoupled. The Cloudflare lesson is that the two layers evolve on independent timelines and sharing code across them creates brittle coupling.
- We should enforce an IPv6-extension-header parse budget in our L4 path (even in simulation) — Unimog's lesson that adversaries will exploit the parser.
- We could write a conformance-regression test harness that replays a corpus of requests across both an old and new binary and diffs response bytes, mirroring Cloudflare's pre-Pingora migration approach.

## Sources
- https://blog.cloudflare.com/unimog/ — "Unimog: Cloudflare's edge load balancer" (2020).
- https://blog.cloudflare.com/how-we-built-pingora-the-proxy-that-connects-cloudflare-to-the-internet/ — Pingora design.
- https://blog.cloudflare.com/magic-transit/ — Magic Transit architecture.
- https://blog.cloudflare.com/introducing-spectrum/ — Spectrum TCP/UDP proxy.
- https://blog.cloudflare.com/how-we-built-quicksilver/ — Quicksilver configuration KV.
- https://blog.cloudflare.com/meet-gatebot-a-bot-that-allows-us-to-sleep/ and https://blog.cloudflare.com/dosd-a-new-go-approach-to-cloudflares-ddos-mitigation/ — DDoS control plane.
- https://blog.cloudflare.com/stopping-http2-rapid-reset-ddos-attacks/ — the rapid-reset mitigation walkthrough.
- https://blog.cloudflare.com/ja4/ — JA4 fingerprinting.
- https://developers.cloudflare.com/load-balancing/ — customer-facing LB product docs.
- https://blog.cloudflare.com/ipv6-extensions/ — extension-header-abuse discussion.
