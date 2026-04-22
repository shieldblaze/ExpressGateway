# Envoy

## Why we study it
Envoy is the industry's most feature-complete L7 proxy, and — more importantly — the reference implementation of the xDS control-plane protocol (CDS/EDS/LDS/RDS/SDS). Anything Envoy has in production has been through a large, adversarial user base; its design decisions and its postmortems are a canonical syllabus for L7 proxies. Our separation of data plane (`lb-l7`) and control plane (`lb-controlplane`) mirrors Envoy's, even though we use a much smaller protocol.

## Architecture in brief
Envoy is C++14 (moving toward C++17/20) built on top of libevent and a handful of carefully chosen libraries (HTTP/2 via nghttp2, QUIC via quiche, TLS via BoringSSL). It is structured around a single concept: the listener. A listener binds a socket, applies listener filters (TLS inspector, TLS termination), then hands the connection into a network filter chain (`envoy.filters.network.*`) that may terminate HTTP and pass requests to the HTTP connection manager (HCM). HCM drives a second, HTTP-specific filter chain (`envoy.filters.http.*`) — routing, auth, rate-limit, compression, lua, wasm — culminating in a router filter that picks a cluster, selects a host, and issues the upstream request.

The data plane is single-writer-per-worker. Each worker thread owns a libevent loop, its own copy of runtime-settings, its own listener sockets (via `SO_REUSEPORT`). There is no cross-worker sharing of the hot path. State that must be global (cluster membership, route tables, runtime feature flags) is maintained by a single "main thread" and distributed via a thread-local-storage abstraction (`Envoy::ThreadLocal::Slot`) with copy-on-update semantics.

Cluster membership comes from xDS: a gRPC bidirectional stream to a control plane (Istio, go-control-plane, Consul, in-house). CDS announces clusters, EDS announces endpoints per cluster, LDS announces listeners, RDS announces routes, SDS announces secrets. ADS ("aggregated discovery service") muxes all of them over a single stream for ordering guarantees. The xDS model is push-with-ack: the management server pushes, Envoy ACKs the version it accepted, or NACKs with an error string.

Storage is in-process; there is no durable Envoy-side state except stats and access logs. Stats use a carefully tuned flat-memory model (the infamous `Envoy::Stats::StatName` symbol table) so that millions of per-host counters don't blow the process.

Concurrency model: N workers + 1 main + blocking pool. Workers are non-blocking, event-loop driven. The main thread handles xDS, admin, and stats scrapes. Overload manager runs its own timer and applies "actions" (stop accepting, shed load, close idle connections) when resource monitors (memory, fd count) breach configured thresholds.

Language: C++ with strict code-review gating. Large surface area: ~1M lines. Contrib filters are sandboxed behind a build flag.

## Key design patterns
1. **Two-stage filter chain (L4 network filters + L7 HTTP filters).** Separates protocol termination from request-level policy so the same HTTP filters compose over HTTP/1, HTTP/2, HTTP/3 uniformly.
2. **Thread-local snapshots of xDS state.** Main thread assembles a new cluster manager, posts it to each worker; worker swaps local pointer on next event-loop iteration. No locks in the request path.
3. **xDS protocol as config substrate.** Decouples control-plane vendor from data-plane binary. NACK-with-reason means bad configs don't poison workers.
4. **Overload manager with graded actions.** Monitors (heap size, active connections) trigger actions (reject 1% of requests, stop accepting, close idle H2 streams). Published in `source/common/overload/`.
5. **Outlier detection and panic-mode EDS.** If too many hosts are unhealthy, Envoy enters panic mode and load-balances across all hosts regardless of health — degraded is better than zero.
6. **Connection pool per cluster, per-worker, per-protocol.** No cross-thread pool lookups. A cluster appears as N independent pools, one per worker; this is sometimes suboptimal but always simple.
7. **Retry budget, not retry count.** Rather than "retry 3 times," Envoy caps retries as a percentage of total active requests, preventing retry storms during partial outages.
8. **Circuit breakers per cluster, per priority.** Max connections, max pending requests, max requests, max retries, max concurrent shadow requests — each is a separate budget.
9. **Hot restart via Unix domain socket parent-child handshake.** New Envoy connects to old Envoy's admin socket, inherits listening fds, signals old to drain; old exits after deadline.
10. **Access log with structured format and per-request dynamic metadata.** Filters can decorate the request with `StreamInfo` key-values that the access log formatter reads.

## Edge cases / hard-won lessons
- **Overload manager must throttle xDS updates too.** Under memory pressure, swapping in a large EDS push can tip the process over. Envoy added "overload-aware xDS" to defer non-essential updates.
- **HTTP/2 rapid reset (CVE-2023-44487).** Attacker opens and RST_STREAMs thousands of streams; each allocation outran the garbage collection. Fix: account reset streams against `max_concurrent_streams`, not just active ones.
- **HPACK bomb.** A malicious header table update can force the decoder to allocate gigabytes. Envoy caps table size per RFC and rejects updates that would exceed it.
- **CONTINUATION flood.** HEADERS + unlimited CONTINUATION frames let attacker consume CPU with no stream created. Fix: cap total header-block size before the frame sequence completes.
- **`x-envoy-original-path` injection** from untrusted clients could cause route mismatches; forwarded-header sanitisation is a per-listener policy.
- **Zone-aware routing falls apart under skewed traffic.** If the local zone has 1 replica and receives 80% of traffic, zone-local mode overloads it. Envoy's solution: minimum-replica threshold before zone-locality kicks in.
- **Slow-header attack (Slowloris).** Per-request header-read deadline needed separately from connection idle timeout.
- **gRPC trailers-only responses** break naive HTTP/2 clients; Envoy detects and rewrites when bridging to HTTP/1.
- **TLS certificate rotation must not reset existing connections.** SDS pushes a new cert; existing connections continue with the old cert, new accepts use the new cert. Out-of-band ticket rotation is independent.
- **`Connection: close` from upstream needs cluster-specific pool eviction** — otherwise the same stale connection is reused across workers via pool sharing (which is why Envoy abandoned pool sharing).
- **Access log PII leakage.** Headers like `authorization` must be redacted by default; contributors have shipped "log all headers" fields that ship tokens to SIEM.
- **CEL/Lua filters CPU attack.** User-supplied expressions can loop; Envoy imposes CPU-time budgets per filter invocation.

## Mapping to ExpressGateway
| Envoy pattern | Our equivalent | Gap / status |
| --- | --- | --- |
| Two-stage filter chain | `crates/lb-l7/src/lib.rs` (bridging modules) + `crates/lb-security/src/lib.rs` (request-level) | We have dedicated bridge modules per protocol pair and a separate security crate; no user-extensible filter chain. |
| xDS config substrate | `crates/lb-controlplane/src/lib.rs` (`ConfigBackend` trait) + `crates/lb-cp-client/src/lib.rs` | We use a file-backed/in-memory trait rather than gRPC xDS. Functionally similar swap-on-update via ArcSwap. |
| Overload manager | `crates/lb-security/src/slowloris.rs`, `slow_post.rs` | Per-attack mitigations exist; a unified overload manager does not. |
| Outlier detection / panic mode | `crates/lb-health/src/lib.rs` | Active health checks present; panic-mode fallback is a policy gap. |
| Retry budget | `crates/lb-core/src/policy.rs` | Policy types present; budget semantics (percentage-based) not formalised. |
| Circuit breakers per cluster | `crates/lb-core/src/cluster.rs` | Cluster abstraction present; breaker state not encoded. |
| Hot restart via fd inheritance | `crates/lb-controlplane/src/lib.rs` | SIGHUP reload present; fd inheritance to a replacement binary is not implemented. |
| HTTP/2 rapid-reset / CONTINUATION / HPACK bomb | `tests/security_rapid_reset.rs`, `tests/security_continuation_flood.rs`, `tests/security_hpack_bomb.rs`, `crates/lb-h2/src/security.rs` | Present; direct Envoy-inspired mitigations. |
| QPACK bomb (H3 analogue) | `tests/security_qpack_bomb.rs`, `crates/lb-h3/src/security.rs` | Present. |
| Access log per request | `crates/lb-observability/src/lib.rs` | Metrics and traces present; structured access-log format with PII redaction is a gap. |
| Thread-local snapshot via ArcSwap | `crates/lb-core/src/cluster.rs`, `crates/lb-controlplane/src/lib.rs` | Same pattern (ArcSwap), scaled to our smaller config. |

## Adoption recommendations
- We should introduce a central `OverloadManager` in `lb-core` with pluggable `ResourceMonitor` traits (heap, fd, active connections) and graded actions (reject, shed, close-idle) — this generalises the per-attack mitigations in `lb-security`.
- We could formalise `RetryBudget` in `lb-core/src/policy.rs` as a percentage ceiling on concurrent retries, not just a max-count.
- We should add `CircuitBreaker` state to `lb-core/src/cluster.rs` (max-connections, max-pending, max-requests) so the balancer can short-circuit before dispatching.
- We could adopt Envoy's panic-mode semantics in `lb-health`: if healthy backends fall below a floor, balance over all backends rather than none.
- We should formalise the PII redaction policy in `lb-observability` (structured-log field redaction list) before we ship any access-log format.
- We could consider an xDS subset (CDS + EDS over gRPC) as an additional `ConfigBackend` implementation if we ever need to interoperate with an Envoy control plane.
- We should keep the `lb-l7` bridge modules as hand-written per protocol-pair rather than a generic filter chain; Envoy's generic chain is a maintenance burden that a four-crate bridge model avoids.
- We should audit every `Err(...)` return in `lb-security` to ensure we account for attempted allocations in whatever counter drives overload (Envoy's rapid-reset lesson: the attack exists in the gap between "stream rejected" and "memory accounted for").

## Sources
- https://github.com/envoyproxy/envoy — source tree, especially `source/common/http/`, `source/common/upstream/`, `source/common/overload/`, `source/common/config/xds_*`.
- https://www.envoyproxy.io/docs/envoy/latest/ — user documentation.
- https://blog.envoyproxy.io/lyfts-envoy-dogfooding-higher-order-network-services-bfd5fe25cbaf — early architecture notes by Matt Klein.
- https://github.com/envoyproxy/envoy/security/advisories — the running catalog of HTTP/2 and smuggling CVEs Envoy has patched.
- https://blog.cloudflare.com/http2-rapid-reset-attack/ — the CVE-2023-44487 writeup that Envoy and Pingora both patched.
- https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/operations/overload_manager/overload_manager — overload manager design.
- https://github.com/cncf/xds — xDS protocol spec, maintained at CNCF.
- Matt Klein, "Service mesh data plane vs. control plane," 2017 (blog); a foundational read for the dichotomy.
