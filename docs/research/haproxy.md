# HAProxy

## Why we study it
HAProxy is the most battle-tested open-source load balancer for TCP and HTTP, in continuous production since 2001. Willy Tarreau's release notes and CVE disclosures are an unparalleled corpus of proxy-hardening lore — smuggling, HPACK, H2 state machines, idle connections, TLS session management — much of which the rest of the industry later re-learned. For L7 and L4 reverse-proxying specifically, HAProxy's feature list and its bug history are our syllabus.

## Architecture in brief
HAProxy is a single C binary (historically single-process single-thread; since 1.8 multi-threaded; since 2.0 multi-worker). It uses a custom event loop over `epoll` / `kqueue` with its own fd scheduler (`src/fd.c`) and a custom task runqueue (`src/task.c`). It is zero-copy where it can be (splicing between sockets when TLS is not in the path) and uses its own buffer ring (`src/buf.c`).

The configuration is a declarative, typed DSL: frontends, backends, listens, defaults, with ACLs as the policy language. ACLs compose into conditions that gate rules (`http-request deny if { ... }`). There is no Turing-complete scripting in core, though Lua is available as an optional engine with time-budget enforcement.

The data plane is streams-over-sessions. A session is a client-facing connection; it can host multiple streams (HTTP/2 multiplexing). Each stream is a state machine that moves through request-analyse, backend-selection, connect, response-analyse, shutdown phases. State transitions drive the scheduler; no thread blocks on I/O.

Upstream protocol support covers HTTP/1, HTTP/2, HTTP/3 (QUIC via a homegrown stack since 2.6), raw TCP, FastCGI. TLS is OpenSSL (or WolfSSL / AWS-LC builds). Health checks are either in-band (HTTP response codes) or agent-check (a separate probe protocol).

Multi-threading since 1.8 is careful: each thread has its own scheduler and task queue, but they share fd tables and can migrate tasks under back-pressure. Multi-worker since 2.0 runs N processes, each multi-threaded; master handles reload via `SO_REUSEPORT` handover.

Graceful reload uses `-sf <old-pid>`: the new binary opens listening sockets with `SO_REUSEPORT`, then signals the old process to stop accepting and drain. Zero-downtime.

Stick tables are HAProxy's in-memory shared-state primitive: LRU key-value maps for rate limits, session affinity, anti-abuse counters. They can be synchronised across peers over TCP (the "peers" protocol) for cluster-wide rate limiting.

Language: C99, with a small assembly and SIMD budget for hot paths (hash functions, header scanning).

## Key design patterns
1. **Stream state machine, not callback chain.** Each stream is a discrete state; the scheduler advances states. Easier to reason about than a deep callback stack — especially when debugging.
2. **Typed config language with ACL composition.** Rules bind to named ACLs; ACLs are evaluated lazily and short-circuit. Reduces the "regex in config" surface.
3. **Splicing when possible, buffered when not.** TCP mode without TLS uses kernel splice for zero-copy; HTTP mode with header rewriting uses a buffer ring.
4. **Stick tables as a first-class primitive.** Rate limiting, geo-blocking, session stickiness, all share one mechanism. Peers-sync replicates state across HAProxy instances.
5. **`option redispatch` for retry-on-another-backend.** Orthogonal to retry-count; lets you say "if the preferred backend fails, try any other" with well-defined semantics.
6. **Per-backend queueing.** A backend can be slower than its fair share; HAProxy queues requests and tracks queue depth as a balance-input.
7. **Connection slots (`maxconn`) per backend and per server.** Hard cap prevents pile-up; queued requests wait until a slot frees or the queue timeout expires.
8. **H2 frame-level accounting.** Per-stream and per-connection windows, per-stream state machine, `tune.h2.max-concurrent-streams` as an explicit knob.
9. **Master-worker with seamless reload.** `-sf` fd-handover or `SO_REUSEPORT` with staggered drain.
10. **Runtime API over a Unix socket.** Live config introspection, enable/disable servers, drain without reload; invaluable for ops.

## Edge cases / hard-won lessons
- **CL-TE and TE-CL smuggling (CVE-2021-40346, and the earlier multi-CVE cluster).** HAProxy 2.3 had a subtle off-by-one in CL parsing that let `Content-Length:<TAB>value` bypass validation and desync downstream. Lesson: parse headers per the exact ABNF, reject on any deviation.
- **HTTP/2 rapid reset (CVE-2023-44487).** Patched in 2.6.15 / 2.7.10 / 2.8.3. Stream-reset accounting must include not-yet-created streams.
- **HPACK bomb.** Header table size updates from the peer must be bounded.
- **Keep-alive pool and `option http-server-close`** interaction: if the backend sends `Connection: close` and the pool ignores it, next request picks a dying socket and fails. HAProxy tracks per-socket close-intent.
- **Stick-table LRU eviction under attack.** A spray of unique source IPs evicts real clients' counters; solved with per-source-address-and-port scoping and sized stick tables.
- **Peers protocol split-brain.** If two HAProxy instances disagree on a counter, the higher wins on merge — but a partition followed by merge can briefly undercount. Acceptable for rate limiting; not for session affinity.
- **Graceful reload with `SO_REUSEPORT` can drop connections if the old process's backlog isn't drained**. Lesson: wait for `accept()` to return `-EAGAIN` before SIGTERM to old.
- **OpenSSL session-cache contention** across threads was a scalability wall until HAProxy moved to per-thread caches with out-of-band synchronisation.
- **`timeout tunnel`** (long-lived WebSocket) independent of `timeout http-request` and `timeout server` — without the tunnel timeout, a 5-minute idle timeout kills WebSockets.
- **Health-check flap.** A single failed check flipping a backend out repeatedly causes thrash; `rise` / `fall` thresholds and slow-start mitigate.
- **Stats socket exposure.** The runtime API can drain servers or dump secrets; must be bound to a trusted socket or protected with Unix permissions.
- **DNS resolver TTL compliance.** HAProxy historically cached DNS aggressively; since 1.8 there's a proper resolver with SRV support, but misconfigurations still bite.
- **Slow POST bodies.** `timeout http-request` covers headers; body needs a separate budget.

## Mapping to ExpressGateway
| HAProxy pattern | Our equivalent | Gap / status |
| --- | --- | --- |
| Stream state machine | `crates/lb-l7/src/*.rs` bridge modules | Our bridges are structured as state progressions per protocol pair; equivalent idea, typed in Rust. |
| Typed config | `crates/lb-config/src/lib.rs` | Rust types; ACL-style composition is out of scope. |
| CL-TE / TE-CL smuggling defence | `crates/lb-security/src/smuggle.rs`, `tests/security_smuggling_cl_te.rs`, `tests/security_smuggling_te_cl.rs` | Present; tests exercise the HAProxy CVE patterns. |
| H2 rapid reset defence | `crates/lb-h2/src/security.rs`, `tests/security_rapid_reset.rs` | Present. |
| HPACK bomb defence | `crates/lb-h2/src/hpack.rs`, `tests/security_hpack_bomb.rs` | Present. |
| QPACK bomb defence (H3) | `crates/lb-h3/src/qpack.rs`, `tests/security_qpack_bomb.rs` | Present. |
| Slowloris / slow-POST defence | `crates/lb-security/src/slowloris.rs`, `slow_post.rs` | Present. |
| `redispatch` / retry to another backend | `crates/lb-core/src/policy.rs` | Policy layer exists; explicit redispatch semantics could be documented. |
| `maxconn` per backend | `crates/lb-core/src/cluster.rs` + `crates/lb-balancer/src/least_connections.rs` | Least-connections balancer tracks counts; a hard cap with queueing is not yet formal. |
| Stick tables | `crates/lb-balancer/src/session_affinity.rs` | Session affinity present; generalised stick-table for rate-limit is a gap. |
| Graceful reload via SIGHUP/fd | `crates/lb-controlplane/src/lib.rs` | Present; ArcSwap-based. |
| Runtime admin socket | `crates/lb-observability/src/lib.rs` | Metrics present; live admin API is a gap. |
| H2 per-connection / per-stream windows | `crates/lb-h2/src/frame.rs` | Frame crate implements window accounting. |
| Splicing vs buffering | `crates/lb-io/src/lib.rs` | Rust async pipeline; kernel-splice is not a current optimisation. |

## Adoption recommendations
- We should formalise a `MaxConn` + per-backend queue in `lb-core`, mirroring HAProxy's hard-cap-plus-queue pattern. The least-connections balancer already tracks counts; the missing piece is the queue with a timeout.
- We could extend `crates/lb-balancer/src/session_affinity.rs` toward a generalised stick-table abstraction usable by rate-limit primitives in `lb-security` (e.g. slow-POST, per-source quotas).
- We should document a `Redispatch` policy flag in `lb-core/src/policy.rs` so operators can say "on upstream connect failure, pick a different backend" unambiguously. The failure-class distinction (connect vs read) must be explicit.
- We should add a `timeout_tunnel` equivalent in H1 / H2 / H3 configs so WebSockets and H2 CONNECT streams aren't killed by the request timeout.
- We could add a live admin API (unix-socket, authenticated) in a future crate for ops parity with HAProxy's runtime API; it is a serious operational gap without it.
- We should add explicit per-server slow-start weighting to the weighted-round-robin balancer (`crates/lb-balancer/src/weighted_round_robin.rs`); HAProxy's slow-start prevents thundering-herd when a flapped backend returns.
- We should audit every smuggling-adjacent header parser against the exact ABNF — HAProxy's long CVE history says the tab character, the horizontal whitespace set, the duplicate-header handling are the attack surface.
- We should bind any admin/observability socket to Unix-domain with filesystem permissions by default (HAProxy stats-socket lesson).

## Sources
- https://www.haproxy.org/ and https://www.haproxy.org/#docs — canonical documentation.
- https://git.haproxy.org/?p=haproxy.git;a=summary — source tree; `src/mux_h1.c`, `src/mux_h2.c`, `src/stick_table.c`, `src/task.c`, `src/fd.c`.
- https://www.haproxy.com/blog — vendor blog by the core developers.
- Willy Tarreau's release announcements (haproxy@formilux.org / https://www.mail-archive.com/haproxy@formilux.org/) — the richest corpus of hard-won-lesson narrative in any proxy project.
- https://nvd.nist.gov/vuln/detail/CVE-2021-40346 — integer overflow in HAProxy HTTP/1 parser (smuggling).
- https://nvd.nist.gov/vuln/detail/CVE-2023-44487 — HTTP/2 rapid reset, multi-proxy.
- https://www.haproxy.com/blog/announcing-haproxy-2-6 — QUIC/H3 integration rationale.
- https://www.haproxy.com/documentation/haproxy-runtime-api/ — runtime API surface.
