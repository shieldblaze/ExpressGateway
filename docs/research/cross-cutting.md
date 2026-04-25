# Cross-Cutting Themes from External Research

Companion to the per-system research notes (`pingora.md`, `envoy.md`,
`haproxy.md`, `nginx.md`, `katran.md`, `cloudflare-lb.md`, `aws-alb-nlb.md`).
Those documents describe what each reference system does; this
document synthesises the recurring themes — the problems every
production-grade reverse proxy or L4 load balancer has had to solve —
and maps each theme to where the solution lives (or should live) in
ExpressGateway.

## Theme 1: connection pool hygiene

Every reviewed system maintains a per-origin connection pool and
every one of them has been bitten by the same failure mode: stale
connections being handed out after the origin has silently closed
its half of the FIN. nginx's `proxy_http_version 1.1; proxy_set_header
Connection ""` idiom plus `keepalive` directive papers over this by
opportunistically retrying idempotent requests; haproxy's `option
http-server-close` side-steps the problem by not pooling at all;
envoy and pingora both implement *connection age* and *max requests
per connection* limits with background probing. The common lesson:
the pool owner must measure idle time, total age, and requests
served, and retire on whichever comes first, with a jittered offset
to avoid thundering herds.

ExpressGateway's `crates/lb-l7` (not yet implemented) must implement
all three limits plus active health checks driven by `crates/lb-health`.
A future ADR (`docs/decisions/`) on connection-pool semantics should
record the retire-thresholds and the probe cadence.

## Theme 2: buffer pooling and allocation hotspots

nginx builds around a slab allocator per-connection; haproxy has a
custom pool allocator; envoy leans on `Buffer::OwnedImpl` plus
jemalloc; pingora uses tokio's default allocator plus `bytes::Bytes`
reference counting; katran is allocation-free on the hot path
because the BPF program has no heap at all. The recurring lesson is
that the hot path must not allocate per request, and that buffer
ownership must be cheap to transfer (move, not copy).

ExpressGateway uses `bytes::Bytes` / `BytesMut` everywhere on the
data path: the chunked decoder (`crates/lb-h1/src/chunked.rs`), the
H2 frame codec (`crates/lb-h2/src/frame.rs`), and the H3 frame
codec (`crates/lb-h3/src/frame.rs`). `BytesMut::split_to` and
`Bytes::slice` do the zero-copy carving. Known gap: we do not yet
have a shared `BytesMut` pool for incoming TLS records; each
connection allocates its own read buffer. Adopting a `linked-list`
free-list per worker thread (tokio-local `RefCell<Vec<BytesMut>>`)
would cut the allocation rate proportionally to active connections.

## Theme 3: flood protection is per-protocol, not one knob

A single "max requests per second" knob is not sufficient defense,
because each protocol has its own asymmetric DoS vector. Rapid
Reset (HTTP/2 RST_STREAM flood, CVE-2023-44487) bypasses any
HTTP-level rate limiter because the attacker never *completes* an
HTTP request. CONTINUATION flood (CVE-2024-27316) looks like a
single request in flight. Slowloris, at the other end, looks like
a legitimate slow client. Envoy, nginx, and Apache all had to ship
targeted mitigations for each class.

ExpressGateway's defenses are intentionally fragmented along the
same lines: `lb-security::SlowlorisDetector`,
`lb-security::SlowPostDetector`, `lb-security::SmuggleDetector`,
`lb-h2::RapidResetDetector`, `lb-h2::ContinuationFloodDetector`,
`lb-h2::HpackBombDetector`, `lb-h3::QpackBombDetector`,
`lb-security::ZeroRttReplayGuard`. Each is independently
parameterised, independently tested
(`tests/security_*.rs`), and independently disabled via config.
This is the correct posture; see `docs/research/dos-catalog.md` for
the full attack-to-defense map.

## Theme 4: config hot-reload vs graceful upgrade

There are two schools: SIGHUP reload (nginx, haproxy) drops the
running process and rereads config, typically using a master/worker
split so workers are recycled gracefully; fd-passing upgrade
(envoy hot restart, haproxy `reload` via socket) hands listening
fds from the old process to the new one over a Unix socket so
established connections survive a full binary upgrade. The former
is simpler; the latter is operationally superior for long-lived
connections (HTTP/2, WebSocket, gRPC streaming).

ExpressGateway's control plane (`crates/lb-controlplane`, per the
project memory snippet, uses `ArcSwap` plus file-backed
`ConfigBackend` trait plus SIGHUP polling) is at the nginx end of
the spectrum today. The long-term target, documented in PROMPT.md
§9 and reserved for `docs/decisions/ADR-0009-graceful-reload.md`,
is an envoy-style hot restart with fd-passing. The architectural
implication is that all listener state must be exportable via `SCM_RIGHTS`.

## Theme 5: TLS ticket-key rotation

TLS session resumption via session tickets (RFC 5077, RFC 8446 §4.6.1)
requires a symmetric key known to any server in the fleet that might
accept a resumed connection. If the key never rotates, captured
tickets remain decryptable forever; if it rotates too often, cache
hit rate collapses and resumption degrades to full handshakes.
Cloudflare's documented cadence is hourly rotation with a two-hour
overlap; AWS ALB documents similar. nginx, haproxy, and envoy all
expect the operator to wire an external rotator; pingora integrates
its own.

ExpressGateway's TLS layer will use rustls (PROMPT.md §8). rustls
exposes `TicketKey` via `ServerConfig::ticketer`; the rotation
schedule is our responsibility. Plan: implement a `TicketRotator`
in `crates/lb-security` that holds a small ring of `[u8; 48]` keys,
rotates hourly from a seed stored in the control plane, and
ArcSwap-publishes the new `Ticketer` to all live `ServerConfig`s.
This is not yet implemented; it is a known gap.

## Theme 6: DNS negative-TTL and dynamic endpoint refresh

A proxy that resolves backend DNS at config-load time and never
refreshes is broken in a Kubernetes / service-mesh world. nginx
under `resolver` with `valid=Ns` correctly re-resolves; envoy's
`STRICT_DNS` cluster does the same. The subtle failure mode is
*negative* caching: a transient NXDOMAIN gets pinned until the
negative TTL expires, and the proxy blackholes traffic even after
the real endpoint returns. RFC 2308 specifies negative-cache
handling; in practice operators reduce negative TTLs to seconds.

ExpressGateway's backend resolver must honour the shorter of the
configured refresh interval and the record TTL, re-resolve on
health-check failure, and cap negative TTLs at 5 seconds. Owning
crate: `crates/lb-health` plus a resolver abstraction in
`crates/lb-l7`. Not yet implemented.

## Theme 7: observability cardinality

Every system reviewed started with per-backend metrics, ended up
with per-backend × per-status-code × per-route × per-cluster
cardinality, and then had a production outage caused by Prometheus
OOM. The lesson is that the *label set* shipped by the proxy must
be constrained at emission time, not at scrape time. Envoy gates
this with `stats.max_tagged_values`; pingora emits a flat counter
and expects the scraper to aggregate.

ExpressGateway's `crates/lb-observability` should expose counters
with an explicit enum of allowed label combinations, not free-form
strings. This means every new label requires a code change, which
is the cost of predictable cardinality. A future ADR should record
the policy.

## Theme 8: backpressure end-to-end (kernel → tokio → app)

Backpressure must propagate unbroken from the kernel's TCP receive
window through the runtime's socket read to the application's
request queue. A broken link anywhere — an unbounded `mpsc`
channel, a `spawn` with no back-signal, a `write_all` that stalls
a shared connection — turns bursty traffic into unbounded memory
growth. haproxy uses a strict single-task-per-connection model that
naturally propagates backpressure. envoy uses watermark buffers
with explicit high/low callbacks. nginx uses epoll readiness to
keep the problem at the fd level. pingora (tokio-based, like us)
uses `tokio::sync::mpsc` with bounded capacity.

Our approach matches pingora's. PROMPT.md §7 pins the write-buffer
watermarks at 64 KiB high / 32 KiB low; the HTTP/1.1 and HTTP/2
stream handlers (when implemented) must pause reads on the client
socket when the *backend* write buffer crosses the high watermark.
`tokio-util::CancellationToken` propagates the "give up" signal.
See `docs/research/tokio-io-uring.md` for the runtime-level
mechanism.

## Priority adoption list for ExpressGateway

Actionable items derived from the themes above, ordered by impact:

1. **Connection pool limits in `crates/lb-l7`.** Implement max-age,
   max-requests, idle-timeout with jitter. Owner: `lb-l7`.
2. **HTTP/1.1 `max_header_bytes` enforcement.** Wire a per-connection
   buffer cap in `crates/lb-io` and consult it in
   `crates/lb-h1/src/parse.rs`. Closes the residual risk in
   `dos-catalog.md` §1 ("no explicit header-byte cap"). Owner:
   `lb-io`, `lb-h1`.
3. **HTTP/2 SETTINGS / PING rate limiters.** Add detectors next to
   `RapidResetDetector` in `crates/lb-h2/src/security.rs`. Owner:
   `lb-h2`.
4. **TLS ticket rotation.** `TicketRotator` in `crates/lb-security`,
   hourly cadence, ArcSwap-published. Owner: `lb-security`.
5. **DNS resolver with negative-TTL cap.** In `crates/lb-health` or
   a new `lb-resolve` crate. Owner: `lb-health`.
6. **Per-worker `BytesMut` pool.** Cheap free-list in `crates/lb-io`
   to cut per-request allocations. Owner: `lb-io`.
7. **Retry budget with jitter in `lb-l7`.** Prevent retry storms
   (residual risk 5 in `dos-catalog.md`). Owner: `lb-l7`.
8. **Metrics label enum in `lb-observability`.** Bound cardinality
   at compile time. Owner: `lb-observability`.
9. **Loom suite for `lb-controlplane` ArcSwap publisher and
   upcoming bounded channel helpers.** Owner: workspace root
   `tests/` plus per-crate `tests/`.
10. **Real eBPF XDP object.** Move `crates/lb-l4-xdp/ebpf/` from
    stub to a working aya-compiled program. Owner: `lb-l4-xdp`.
    See `aya-ebpf.md`.

## Sources

- nginx reference, https://nginx.org/en/docs/
- haproxy documentation, https://docs.haproxy.org/
- Envoy configuration reference,
  https://www.envoyproxy.io/docs/envoy/latest/configuration/configuration
- Cloudflare Pingora blog series,
  https://blog.cloudflare.com/pingora-open-source/
- Cloudflare "How we built Pingora",
  https://blog.cloudflare.com/how-we-built-pingora-the-proxy-that-connects-cloudflare-to-the-internet/
- Facebook/Meta Katran, https://github.com/facebookincubator/katran
- AWS ALB / NLB documentation,
  https://docs.aws.amazon.com/elasticloadbalancing/
- RFC 5077 (TLS session resumption),
  https://www.rfc-editor.org/rfc/rfc5077
- RFC 2308 (Negative DNS caching),
  https://www.rfc-editor.org/rfc/rfc2308
- ExpressGateway internal docs: `dos-catalog.md`,
  `tokio-io-uring.md`, `aya-ebpf.md`, `PROMPT.md` (§2, §7, §9, §26).
