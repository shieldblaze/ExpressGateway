# Cloudflare Pingora

## Why we study it
Pingora is the closest living relative of ExpressGateway: a Rust-native, multi-protocol L7 proxy framework that Cloudflare runs at trillions-of-requests-per-day scale. It has already re-litigated every design decision we are making — connection pooling, graceful reload, failure handling, protocol bridging — and published postmortems of the bugs it found along the way.

## Architecture in brief
Pingora is structured as a set of composable Rust crates (`pingora-core`, `pingora-proxy`, `pingora-pool`, `pingora-load-balancing`, `pingora-http`, `pingora-cache`, `pingora-limits`, `pingora-ketama`, `pingora-error`, `pingora-timeout`, `pingora-memory-cache`). An operator-built binary wires a `Server` (thread/runtime topology + signal handling) to one or more `Service`s, each of which holds a `ProxyHttp` trait object. The trait exposes lifecycle phases (`request_filter`, `upstream_peer`, `upstream_request_filter`, `response_filter`, `logging`, `fail_to_connect`) that the operator implements — a pattern inherited from NGINX modules but typed in Rust.

The data plane uses a work-stealing Tokio runtime (`pingora-core/src/server/mod.rs`) with optional thread-pinned runtimes (`pingora-runtime`). I/O abstracts over TCP, TLS (BoringSSL via `pingora-boringssl` or OpenSSL), Unix domain sockets, and HTTP/1, HTTP/2, and (recently) HTTP/3. The `pingora-proxy` crate owns the request lifecycle and transparently bridges between protocol versions on the downstream and upstream legs. Buffering is opt-in: by default bodies stream in fixed-size chunks, with backpressure propagated through Tokio's `AsyncRead`/`AsyncWrite`.

The upstream pool (`pingora-pool`) is a per-origin LRU of idle sessions keyed by `(SocketAddr, SNI, ALPN, TLS verify mode)`. Before handing a pooled connection back to a caller, Pingora performs a non-blocking probe read — if the peer has half-closed the socket, `poll_read` returns `Ok(0)` and the connection is discarded. This eliminates the "pool returned a dead connection, request failed" class of bugs that plagues NGINX keepalive.

The control plane is deliberately minimal. Pingora does not ship its own xDS-like protocol; instead the `Service` exposes a configuration type and the operator is expected to reload it via SIGHUP, gRPC, file watch, or an in-process admin API. Graceful reload uses `SO_REUSEPORT` fd-passing: the parent `execve()`s a new binary that inherits listening sockets, then the parent drains and exits (`pingora-core/src/server/transfer_fd.rs`).

Storage is per-process: routing tables, upstream pools, and rate-limit token buckets live in `ArcSwap` or lock-free maps (`papaya`, `dashmap`). There is no shared-memory IPC — each worker is independent, and consistency is eventual via the control plane push.

Concurrency model: cooperative tasks on Tokio, one task per accepted connection, plus fan-out tasks for HTTP/2 stream muxing. Blocking work (BoringSSL handshake CPU, regex) runs on a dedicated blocking pool. Shutdown is two-phase: stop accepting, then drain existing requests up to a deadline.

Language: Rust stable with `#![forbid(unsafe_code)]` in most crates; unsafe is concentrated in FFI shims.

## Key design patterns
1. **Phased filter chain.** `ProxyHttp` trait methods are called in a fixed order and each returns a `Result` whose `Err` aborts the chain (`pingora-proxy/src/proxy_trait.rs`). This avoids the "middleware ordering depends on registration" pitfall Envoy has.
2. **Connection-pool probe before reuse.** See `pingora-pool/src/connection.rs`; a pooled session is `read`-probed with a zero-length buffer before being returned. Without this the proxy silently retries on closed connections.
3. **Graceful reload via fd passing.** Parent `execve()`s the new binary, new binary inherits listeners, parent drains. Documented in Cloudflare's "How we built Pingora" (2022) post.
4. **Timeout granularity at every phase.** Separate timeouts for connect, TLS handshake, header read, body read, and total request life (`pingora-timeout`). This is how you bound worst-case latency under attack.
5. **Retry state machine with idempotency guard.** Only idempotent methods retry by default; bodies are buffered (or re-streamed from the origin of record) only when retry is enabled.
6. **Load-balancing set as an `Arc<Backends>` snapshot.** Health updates swap the snapshot; picks are lock-free reads (`pingora-load-balancing/src/lib.rs`). Same pattern used by Envoy EDS.
7. **Error taxonomy with retry class.** `pingora-error::ErrorType` distinguishes connect/TLS/read/write errors, and the proxy applies different retry policies per class.
8. **Opt-in caching with a pluggable storage trait.** `pingora-cache` separates the cache policy from the storage backend so Cloudflare can plug in their proprietary tiered store while OSS users can use memory/disk.
9. **Subrequest isolation.** For cache revalidation or auth, Pingora spawns a subrequest with its own timeout and error budget, rather than blocking the main request task.
10. **Structured logging via per-phase hooks.** `logging()` is the last phase and always runs, even on early abort — guarantees a log line per request even when the origin never answered.

## Edge cases / hard-won lessons
- **Silent half-closed pool connections.** Before the probe-on-reuse was added, about one in a few thousand pool hits would fail because the origin had closed after the idle timeout but the kernel had not yet surfaced RST.
- **HTTP/2 PING starvation.** An upstream that stops reading frames can stall every muxed stream. Pingora enforces per-stream body flow-control deadlines independent of connection-level PING-ACK.
- **Body-buffered retry amplifies memory.** Pingora caps the buffered-retry body size; requests above the cap become non-retryable. Unbounded buffering has OOM'd edge nodes.
- **TLS session-ticket rotation races.** A rotation happening mid-handshake can cause resumption failures; tickets are versioned and old versions kept for a grace window.
- **`SO_REUSEPORT` kernel distribution is hash-based, not round-robin** — an unbalanced 5-tuple (e.g. single heavy client) pins load to one worker. Mitigated by worker CPU accounting and per-worker queue depth metrics.
- **Graceful reload socket leak.** If the new binary crashes between fd-inheritance and `listen_accept`, the parent's drain deadline could pass and leave no listener. Pingora added a reload-health-check probe before the parent exits.
- **Logger being the bottleneck.** Structured JSON logging blocked the request thread during fsync pauses. Fix: bounded ring buffer + dedicated log thread, drop on overflow with a counter.
- **Upstream IPv6 happy-eyeballs misordering.** Without RFC 8305 racing, DNS IPv6-first on a broken v6 path added 5 s per request. Pingora implements per-resolver v4/v6 races.
- **BoringSSL vs. OpenSSL behavioural differences** in error reporting broke a custom error-to-status mapping; the error taxonomy is now TLS-vendor neutral.
- **`Connection: close` from origin must propagate to the pool as "do not reuse"**, not just as a response header, or subsequent requests pick the dying socket.

## Mapping to ExpressGateway
| Pingora pattern | Our equivalent | Gap / status |
| --- | --- | --- |
| Phased `ProxyHttp` trait | `crates/lb-l7/src/lib.rs` bridge modules | We have per-combination bridges, not a single typed trait; cleaner bridges than Pingora for protocol-crossing specifically. |
| Connection-pool probe | `crates/lb-io/src/lib.rs` | Present (connection hygiene); upstream-pool semantics live here. |
| Graceful reload via fd inherit | `crates/lb-controlplane/src/lib.rs` + `crates/lb/src/main.rs` | We implement SIGHUP reload with ArcSwap rollback; fd-inheritance is a possible future extension. |
| Phased timeouts | Per-protocol crates (`lb-h1`, `lb-h2`, `lb-h3`, `lb-quic`) | Timeouts are configured per crate; no unified "timeout budget" type yet. |
| Retry state machine | `crates/lb-core/src/policy.rs` | Policy types exist; idempotency classification lives at the protocol layer. |
| `Arc<Backends>` snapshot | `crates/lb-core/src/cluster.rs`, `crates/lb-balancer/src/*.rs` | Direct match — balancers operate on immutable `&[Backend]` built from a snapshot. |
| Error taxonomy with retry class | `crates/lb-core/src/error.rs`, per-crate `error.rs` | Per-crate taxonomies; retry-class annotation is partial. |
| Maglev consistent hashing | `crates/lb-balancer/src/maglev.rs` | Present, 65537-slot table. |
| Ring hash (Ketama) | `crates/lb-balancer/src/ring_hash.rs` | Present. |
| Structured logging hook that always runs | `crates/lb-observability/src/lib.rs` | Metrics and traces present; "last-phase log guaranteed" contract is not formalised. |

## Adoption recommendations
- We should add a unified `Deadline` / `TimeoutBudget` type in `lb-core` that the per-protocol crates subtract from, so a slow TLS handshake shortens the body-read window rather than both running full duration.
- We could extend `lb-core/src/error.rs` with an explicit `RetryClass` enum (connect / tls / read-headers / read-body / write / status) that `lb-l7` consults before retrying.
- We should document (and test) a connection-pool probe policy even though `lb-io` is currently synchronous-style; the probe-on-reuse invariant must survive any future async-pool refactor.
- We could add a "reload health probe" to `lb-controlplane` — refuse to commit a new config until a dry-run validates it against fixture traffic, mirroring Pingora's reload-safety.
- We should lift the "logging always runs, even on early abort" contract from `lb-observability` into the `lb-l7` bridge modules' `Drop` impls so metrics and access logs cannot be skipped by an early `?`.
- We could adopt the Pingora pattern of a tiny `Service` trait in `lb` that each protocol server implements, rather than ad hoc wiring in `main.rs`.
- We should benchmark `SO_REUSEPORT` hash balance under pathological 5-tuples before committing to it as our listener strategy.

## Sources
- https://github.com/cloudflare/pingora — source tree, especially `pingora-core`, `pingora-proxy`, `pingora-pool`, `pingora-load-balancing`, `pingora-timeout`, `pingora-error`.
- https://blog.cloudflare.com/pingora-open-source/ — 2024 open-source announcement.
- https://blog.cloudflare.com/how-we-built-pingora-the-proxy-that-connects-cloudflare-to-the-internet/ — 2022 design rationale, including the half-closed-pool story.
- https://blog.cloudflare.com/pingora-saving-compared-to-nginx/ — CPU and memory wins vs NGINX, and the architectural reasons.
- https://github.com/cloudflare/pingora/blob/main/docs/user_guide/internals.md — user-guide architecture notes.
- Pingora team presentations at RustConf 2023 and QCon 2023 (talks indexed on YouTube under "Pingora Cloudflare").
