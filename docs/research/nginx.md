# NGINX / OpenResty

## Why we study it
NGINX is the proxy against which every subsequent proxy benchmarks itself. Its event-driven, single-threaded-per-worker design set the template for modern L7 proxies; its configuration language and module API are the ergonomic yardstick. OpenResty's Lua embedding is the canonical cautionary tale in "filter hooks that can block." Pingora explicitly exists because NGINX's module model could not evolve fast enough at Cloudflare scale — understanding NGINX's constraints clarifies why Pingora (and ExpressGateway) make different choices.

## Architecture in brief
NGINX is C99, single-binary, with a master process + N worker processes. The master does configuration parsing, signal handling, and fd distribution; workers handle I/O. Each worker runs a single-threaded event loop (`epoll` on Linux, `kqueue` on BSD) with no shared memory in the hot path. Inter-worker coordination is via `SO_REUSEPORT` listeners (since 1.9.1) or accept-mutex (legacy). There is a dedicated cache-manager and cache-loader process for disk-backed caches.

The request lifecycle is organised into eleven phases (`ngx_http_core_module`): post-read, server-rewrite, find-config, rewrite, post-rewrite, preaccess, access, post-access, try-files, content, log. Modules hook into specific phases. The phased model predates Envoy's filter chain by a decade and influenced it directly.

Upstream selection is module-driven: `ngx_http_upstream_module` defines the `peer.init`/`peer.get`/`peer.free` ABI; specific balancers (`ip_hash`, `least_conn`, `hash`, `random`, third-party `ewma`) implement it. Keepalive to upstreams is opt-in via `keepalive` directive; without it each request opens a new upstream socket.

TLS is via OpenSSL by default. HTTP/2 was a first-party implementation (2015). HTTP/3 shipped in 2022 via a `ngx_http_v3_module` built on `quiche` (integration), with the QUIC stack rewritten partially in-tree.

Config is a text DSL: nested blocks, include directives, no first-class loops or conditionals. Reload is SIGHUP: master re-parses, spawns new workers with the new config, signals old workers to drain. This is the canonical graceful-reload pattern every other proxy (including ours) imitates.

Shared memory zones (`ngx_slab`) provide cross-worker state for rate limits (`limit_req`, `limit_conn`), upstream zone state (`upstream...zone`), and the cache index. A hand-rolled slab allocator with inline locks.

Language: C; OpenResty adds LuaJIT via `ngx_http_lua_module`, exposing NGINX internals to Lua coroutines for request-time scripting. OpenResty's non-blocking Lua sockets (cosockets) wrap NGINX's event loop.

## Key design patterns
1. **Worker-per-core, no shared hot path.** Eliminates cross-core cache contention; scales linearly for embarrassingly-parallel traffic.
2. **Phased request processing.** Eleven-phase pipeline lets modules compose without knowing about each other. Filter modules (header-filter, body-filter) form a second chain on the response.
3. **Upstream `peer.get`/`peer.free` ABI.** Load balancers are pluggable behind a minimal C interface; lets third parties ship balancers without patching core.
4. **Keepalive cache per-worker.** Each worker maintains its own LRU of idle upstream sockets; no cross-worker reuse. Same trade-off Envoy later made.
5. **`SO_REUSEPORT` for listen-socket distribution.** Kernel-hashed across workers, replacing the old accept-mutex. Major win for high-connection-rate workloads.
6. **Shared-memory slab allocator for cross-worker state.** `ngx_slab_alloc` with a rwlock; expensive to write, cheap to read.
7. **Request body buffering policies.** `client_body_in_file_only`, `proxy_request_buffering off` etc. let operators trade memory, disk, and end-to-end streaming semantics.
8. **Chunked-transfer-encoding parser that handles trailers, extensions, and invalid input defensively.** The smuggling CVEs taught NGINX to be strict; RFC 9112 §7.1 detail compliance is built in.
9. **Graceful reload via SIGHUP + old-worker drain.** Old workers finish in-flight requests (up to `worker_shutdown_timeout`), new workers handle new requests.
10. **Output filter chain for response transforms** (gzip, sub, addition). Streams byte ranges through filters without full-body buffering.

## Edge cases / hard-won lessons
- **`proxy_request_buffering on` (default) stalls real-time uploads.** Operators who don't flip it off see 10-second delays on large uploads because NGINX buffers the entire body before contacting upstream.
- **`ip_hash` with IPv6 and NAT.** Same client-subnet can hash to different backends over v4 vs v6; use a hash on a known-stable header instead.
- **`proxy_next_upstream` default includes `error timeout`** — which silently retries non-idempotent POSTs on network errors. Has caused duplicate orders in production many times.
- **Slowloris.** NGINX fixed this via `client_header_timeout` and `client_body_timeout`, but the defaults were lax for years; per-connection byte-rate limits (`limit_req`) are needed too.
- **HTTP/2 rapid-reset (CVE-2023-44487).** NGINX added `keepalive_requests` per-connection limits and finer stream-reset accounting.
- **Smuggling CVEs.** Historic CL+TE handling bugs in both NGINX and reverse-proxy deployments (NGINX in front of Node, for example). RFC 9112 §6.1 strictness is the answer.
- **Shared-memory zone corruption on worker crash.** A worker SIGSEGV holding a slab lock wedges future workers. Mitigation: zone versioning and validate-on-read.
- **Lua `ngx.sleep` bypassed the non-blocking contract in early versions.** OpenResty operators had to learn that not every Lua API is cosocket-safe.
- **`resolver` cached NXDOMAIN for the full DNS TTL** even under backend fail-over — recipe for cascading outages. Since fixed with `resolver_timeout` and `valid=` overrides.
- **`sendfile` interacts badly with TLS** — the kernel sendfile path can't encrypt, and NGINX falls back to read-then-write. Unexpected perf cliff on TLS upstreams.
- **`worker_connections` includes upstream sockets** — exhausting it gives misleading "accept failed" errors.
- **Access log is synchronous by default.** `access_log ... buffer=... flush=...` is required for high-RPS; otherwise fsync pauses serialise requests.
- **Gzipping already-compressed content wastes CPU and often breaks clients** — content-type filtering is mandatory, and `gzip_vary` interacts subtly with downstream caches.

## Mapping to ExpressGateway
| NGINX pattern | Our equivalent | Gap / status |
| --- | --- | --- |
| Worker-per-core event loop | `crates/lb-io/src/lib.rs` (Tokio-based) | Tokio's default is multi-threaded work-stealing; we do not currently pin one runtime per core. |
| Phased request lifecycle | `crates/lb-l7/src/lib.rs` per-protocol-pair bridges | Different shape — we bridge H{1,2,3} to H{1,2,3} directly, not via an 11-phase pipeline. |
| Upstream balancer ABI | `crates/lb-balancer/src/lib.rs` (`LoadBalancer` / `KeyedLoadBalancer` traits) | Direct analogue, Rust-typed. |
| Keepalive cache per-worker | `crates/lb-io/src/lib.rs` | Per-process connection handling; a formal keepalive pool is TBD. |
| `SO_REUSEPORT` listener distribution | `crates/lb/src/main.rs` listener setup | Supported via Tokio; see pattern documented in the crate. |
| Shared-memory zones | (none) | Rust process model uses in-process ArcSwap rather than cross-process shm. |
| Request-body buffering policy | `crates/lb-h1/src/chunked.rs`, `crates/lb-h2/src/frame.rs` | Protocol crates implement streaming; buffering policy is a per-protocol setting. |
| Smuggling mitigations | `crates/lb-security/src/smuggle.rs`, `tests/security_smuggling_*.rs` | Present with explicit RFC 9112 §6.1 and RFC 9113 §8.2.2 handling. |
| Slowloris / slow POST | `crates/lb-security/src/slowloris.rs`, `slow_post.rs`, `tests/security_slowloris.rs`, `tests/security_slow_post.rs` | Present. |
| Graceful SIGHUP reload | `crates/lb-controlplane/src/lib.rs` | Present; ArcSwap + rollback. |
| Output-filter response transforms | `crates/lb-compression/src/lib.rs` + `tests/compression_*.rs` | Compression present; general response-transform filter chain is out of scope. |

## Adoption recommendations
- We should document default timeouts explicitly in `lb-h1`/`lb-h2`/`lb-h3` configs — NGINX's lax historical defaults are the #1 lesson.
- We could add an opt-in `client_body_in_file_only` equivalent: backpressure upload bodies to disk if memory exceeds a threshold. Useful for large-upload profiles.
- We should ensure our retry policy in `lb-core/src/policy.rs` does not silently retry non-idempotent methods on network error — the `proxy_next_upstream` footgun.
- We could adopt NGINX's `worker_shutdown_timeout` naming and semantics for our graceful-reload drain deadline so operators recognise it.
- We should guard compression behind a content-type allowlist in `lb-compression` to avoid double-compressing already-compressed bodies (the `gzip_vary` lesson).
- We could benchmark a one-runtime-per-core topology (Tokio `current_thread` runtimes per worker with `SO_REUSEPORT`) against the default multi-threaded runtime; NGINX's empirical result is that per-core worker wins on high-connection workloads.
- We should keep access logs off the request hot path — route through `lb-observability`'s structured sink with bounded buffering, not synchronous fsync.
- We should treat DNS resolution with explicit TTL bounds and negative-cache overrides (NXDOMAIN caching bug).

## Sources
- https://nginx.org/en/docs/ — canonical reference.
- https://nginx.org/en/docs/dev/development_guide.html — module ABI, phase model, upstream interface.
- http://hg.nginx.org/nginx/file/tip/src/http/ — source tree; `ngx_http_upstream.c`, `ngx_http_core_module.c`, `ngx_http_request.c`.
- Igor Sysoev, "NGINX: origin and future" — early architecture talks, indexed on YouTube.
- https://openresty.org/en/ and https://github.com/openresty/lua-nginx-module — OpenResty / Lua integration and cosocket rationale.
- https://blog.cloudflare.com/pingora-saving-compared-to-nginx/ — explicit comparison of NGINX limits vs Pingora design.
- https://nvd.nist.gov/vuln/detail/CVE-2023-44487 — rapid reset across proxies including NGINX.
- http://nginx.org/en/CHANGES — release notes; the best running log of NGINX's hard-won lessons.
