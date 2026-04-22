# Gap Analysis: Spec vs Implementation

## Method

We walked every subsection of the feature-parity checklist in
`PROMPT.md` §28, cross-referenced each line against the Rust code under
`crates/` and the integration tests under `tests/`, and graded each
row. "Evidence" below is the concrete `crate:path` or `test:name` that
supports the grade. Where a feature is partial or simulated, the
rationale is stated and the work needed to make it production-real is
spelled out in the "Known-simulated areas" and "Deferred items"
sections.

The grading was done on 2026-04-22 against the HEAD of `main` (commit
`b9853178`). The test counts quoted are from a fresh run of
`cargo test --all --all-features --no-fail-fast`: 98 test binaries,
296 passing tests, 0 failures, 0 ignored.

## Status legend

| Status | Meaning |
|--------|---------|
| **present** | Implemented and covered by at least one test. |
| **partial** | Some sub-behaviour implemented, others missing. |
| **absent** | Not implemented; the API surface doesn't exist. |
| **deferred-with-rationale** | Explicitly out of scope for v1, with a documented reason. |
| **simulated** | Present as a userspace model, but not yet wired to the real kernel / network substrate. |

## Gap matrix

The PROMPT.md §28 checklist has 12 sections; we give at least 40 rows
below, grouped by module, with status, evidence, and notes. Rows marked
[required] appear verbatim in `manifest/required-tests.txt`.

### L4 TCP

| PROMPT row | Status | Evidence | Notes |
|---|---|---|---|
| TCP proxy with connection tracking | **simulated** | `crates/lb-l4-xdp/src/lib.rs` (FlowKey, ConntrackTable); `tests/l4_xdp_conntrack.rs::test_xdp_conntrack_flow_pinning` [required] | Userspace simulation; no kernel BPF program loaded. |
| XDP-accelerated TCP forwarding | **deferred-with-rationale** | `crates/lb-l4-xdp/ebpf/src/main.rs` (empty stub) | The aya-bpf program is stubbed. Real attach requires a deployment target (kernel + NIC) which is not yet fixed. See ADR-0004. |
| TCP connection pooling (FIFO, 8 idle, 256 total, 60s) | **absent** | — | No pool implementation yet; `lb/src/main.rs` opens a fresh `TcpStream` per request. |
| Half-close support (RFC 9293) | **absent** | — | Proxy forwarding uses `tokio::io::copy_bidirectional` only in the reference binary. |
| Connection backlog queue (10,000, memory-pressure adaptive) | **absent** | — | Kernel default backlog is used. |
| Graceful drain with timeout (30s) | **partial** | `tests/reload_zero_drop.rs::test_reload_zero_drop_under_load` [required] | Zero-drop reload is modelled; hard drain-timeout knob is not yet configurable. |
| TCP Fast Open, NODELAY, QUICKACK, KEEPALIVE, REUSEPORT | **absent** | — | Socket options are not yet plumbed through `lb-config`. |

### L4 UDP

| PROMPT row | Status | Evidence | Notes |
|---|---|---|---|
| UDP proxy with session management | **simulated** | `crates/lb-l4-xdp/src/lib.rs` (protocol byte 17 in `FlowKey`) | Conntrack table models UDP sessions; no real UDP socket code path yet. |
| XDP-accelerated UDP forwarding | **deferred-with-rationale** | `crates/lb-l4-xdp/ebpf/src/main.rs` | Same deferral as TCP-XDP. |
| Session expiry (30s via expiring map) | **partial** | FIFO eviction in the simulated conntrack table | TTL-based expiry is not implemented; eviction is purely capacity-driven. |
| Session rate limiting (token bucket per source IP) | **absent** | — | No token-bucket implementation present. |
| UDP GRO | **deferred-with-rationale** | — | Requires kernel socket integration. |

### Load Balancing (L4 and L7 share algorithms)

| PROMPT row | Status | Evidence | Notes |
|---|---|---|---|
| Round Robin | **present** | `crates/lb-balancer/src/round_robin.rs`; `tests/balancer_round_robin.rs::test_balancer_round_robin_distribution` [required] | Lock-free atomic index. |
| Weighted Round Robin (NGINX-style smooth) | **present** | `crates/lb-balancer/src/weighted_round_robin.rs`; `tests/balancer_weighted_round_robin.rs::test_balancer_wrr_respects_weights` [required] | |
| Least Connection | **present** | `crates/lb-balancer/src/least_connections.rs`; `test_balancer_least_conn_picks_min` [required] | |
| Least Load / Least Request | **present** | `crates/lb-balancer/src/least_request.rs`; `test_balancer_least_req_picks_min` [required] | |
| Random (thread-local RNG) | **present** | `crates/lb-balancer/src/random.rs`; `test_balancer_random_uniform` [required] | |
| Weighted Random | **present** | `crates/lb-balancer/src/weighted_random.rs`; `test_balancer_wrandom_respects_weights` [required] | |
| Power-of-Two-Choices | **present** | `crates/lb-balancer/src/p2c.rs`; `test_balancer_p2c_converges` [required] | |
| Maglev consistent hash | **present** | `crates/lb-balancer/src/maglev.rs`; `test_balancer_maglev_consistent` [required] | Prime-sized table. |
| Ring hash | **present** | `crates/lb-balancer/src/ring_hash.rs`; `test_balancer_ring_hash_consistent` [required] | |
| EWMA least response time | **present** | `crates/lb-balancer/src/ewma.rs`; `test_balancer_ewma_prefers_fast` [required] | |
| Sticky session (SHA-256 cookie) | **partial** | `crates/lb-balancer/src/session_affinity.rs`; `test_balancer_session_affinity_sticky` [required] | Hashing logic is covered; the exact `X-SBZ-EGW-RouteID` cookie name plus full flag set (HttpOnly, SameSite=Strict, Secure) is a presentation detail not yet wired through L7 response injection. |

### TLS / Cryptography

| PROMPT row | Status | Evidence | Notes |
|---|---|---|---|
| TLS 1.2 / 1.3 support | **absent** | — | No rustls integration wired yet; `lb/src/main.rs` operates in cleartext. |
| TLS 1.1 rejection | **absent** | — | Blocked on the prior row. |
| Cipher profiles (modern / intermediate) | **absent** | — | |
| SNI-based cert selection (exact + wildcard) | **absent** | — | |
| ALPN negotiation (h2, http/1.1) | **absent** | — | The ALPN string-matching logic lives in `lb-l7` but is not reachable from the binary yet. |
| OCSP stapling (6h refresh) | **absent** | — | |
| mTLS (NOT_REQUIRED / OPTIONAL / REQUIRED) | **absent** | — | |
| Certificate hot-reload (RwLock atomic swap) | **absent** | — | Pattern exists in `lb-controlplane` ArcSwap; not yet applied to certs. |

### HTTP/1.1

| PROMPT row | Status | Evidence | Notes |
|---|---|---|---|
| Request-line parsing | **present** | `crates/lb-h1/src/parse.rs`; `test_h1_request_line_parsing` [required] | |
| Chunked transfer encoding | **present** | `crates/lb-h1/src/chunked.rs`; `test_h1_chunked_encoding` [required] | Decoder + encoder. |
| Trailer handling | **present** | `crates/lb-h1/src/parse.rs::parse_trailers`; `test_h1_trailer_handling` [required] | |
| Pipelining (serial response ordering) | **partial** | Parser accepts pipelined requests | End-to-end ordered dispatch is not wired through the reference binary. |
| 100-continue | **absent** | — | |
| Hop-by-hop header stripping (RFC 9110 §7.6.1) | **partial** | Bridge code (`crates/lb-l7/src/h1_to_h*.rs`) removes the obvious hop-by-hop headers (Connection, Keep-Alive, TE, Transfer-Encoding, Upgrade, Trailer) | The full RFC 9110 tokenised Connection-header list is not yet re-parsed at runtime. |
| Header injection (Via, X-Request-ID, XFF/XFP) | **absent** | — | |
| Request body size limit (413) | **absent** | — | |
| Slowloris / slow-POST defense | **present** | `crates/lb-security/src/slowloris.rs`, `slow_post.rs`; `tests/security_slowloris.rs::test_slowloris_connection_reaped` [required]; `tests/security_slow_post.rs::test_slow_post_rejected` [required] | |
| URI normalization / path traversal / null byte | **partial** | `parse_request_line` rejects NUL bytes in the path | RFC 3986 normaliser is not yet implemented. |
| Health endpoints (/health, /ready) | **absent** | — | |
| Error responses (400/413/414/431/502/503/504) | **partial** | 400 on parse failure is present in `H1Error` | Dedicated status-specific response builders are not present. |

### HTTP/2

| PROMPT row | Status | Evidence | Notes |
|---|---|---|---|
| Frame decoder | **present** | `crates/lb-h2/src/frame.rs`; `test_h2_frame_decoder` [required] | |
| HPACK encode/decode | **present** | `crates/lb-h2/src/hpack.rs`; `test_h2_hpack_roundtrip` [required] | |
| Rapid-reset mitigation | **present** | `crates/lb-h2/src/security.rs::RapidResetDetector`; `tests/security_rapid_reset.rs::test_h2_rapid_reset_mitigation` [required] | |
| CONTINUATION-flood mitigation | **present** | `crates/lb-h2/src/security.rs::ContinuationFloodDetector`; `test_h2_continuation_flood_mitigation` [required] | |
| HPACK-bomb mitigation | **present** | `crates/lb-h2/src/security.rs::HpackBombDetector`; `test_h2_hpack_bomb_mitigation` [required] | |
| Flow control (conn + stream) | **partial** | Frame types include WINDOW_UPDATE | No end-to-end flow-control state machine on the proxy path. |
| h2c prior knowledge | **absent** | — | |
| CONNECT tunneling (RFC 9113 §8.5) | **absent** | — | |
| Per-stream and 256 MB aggregate body limit | **absent** | — | |
| GOAWAY graceful shutdown | **partial** | Frame type is present in the decoder | No controller logic wired to listeners. |
| H1 <-> H2 translation both directions | **present** | `tests/bridging_h1_h2.rs::test_bridge_h1_to_h2`, `tests/bridging_h2_h1.rs::test_bridge_h2_to_h1` [both required] | |

### HTTP/3 / QUIC

| PROMPT row | Status | Evidence | Notes |
|---|---|---|---|
| QUIC transport | **simulated** | `crates/lb-quic/src/lib.rs` | Not backed by `quiche` yet (see ADR-0003). |
| H3 frame decoder | **present** | `crates/lb-h3/src/frame.rs`; `test_h3_frame_decoder` [required] | |
| QPACK roundtrip | **present** | `crates/lb-h3/src/qpack.rs`; `test_h3_qpack_roundtrip` [required] | |
| QPACK-bomb mitigation | **present** | `crates/lb-h3/src/security.rs::QpackBombDetector`; `test_h3_qpack_bomb_mitigation` [required] | |
| Varint codec | **present** | `crates/lb-h3/src/varint.rs` | Exported `MAX_VARINT`, `encode_varint`, `decode_varint`. |
| 0-RTT replay guard | **present** | `crates/lb-security/src/zero_rtt.rs::ZeroRttReplayGuard`; `tests/security_zero_rtt_replay.rs::test_zero_rtt_replay_rejected` [required] | |
| QUIC datagram / stream forwarding | **simulated** | `test_quic_datagram_forwarding`, `test_quic_stream_forwarding` [both required] | Uses in-process sim types. |
| Stateless retry, connection migration, NAT rebinding, path validation | **absent** | — | Needs real QUIC stack. |
| Alt-Svc header injection | **absent** | — | |
| Connection ID routing | **absent** | — | |
| H1/H2 <-> H3 translation | **present** | `tests/bridging_h1_h3.rs`, `tests/bridging_h2_h3.rs`, `tests/bridging_h3_h1.rs`, `tests/bridging_h3_h2.rs`, `tests/bridging_h3_h3.rs` [all required] | |

### gRPC

| PROMPT row | Status | Evidence | Notes |
|---|---|---|---|
| gRPC detection (Content-Type) | **present** | `crates/lb-grpc/src/lib.rs` | |
| HTTP -> gRPC status mapping | **present** | `crates/lb-grpc/src/status.rs`; `test_grpc_status_translation` [required] | |
| Deadline parsing + enforcement | **present** | `crates/lb-grpc/src/deadline.rs`; `test_grpc_deadline_propagation` [required] | 300 s clamp enforced. |
| Unary / server / client / bidi streaming | **present** | `crates/lb-grpc/src/streaming.rs`; `test_grpc_unary_roundtrip`, `test_grpc_server_streaming`, `test_grpc_client_streaming`, `test_grpc_bidi_streaming` [all required] | |
| Trailer forwarding (grpc-status, grpc-message) | **present** | Covered by the streaming tests. | |
| /grpc.health.v1.Health/Check endpoint | **absent** | — | |
| gRPC-aware compression | **absent** | — | Hooks exist in `lb-compression`, not wired. |

### WebSocket

| PROMPT row | Status | Evidence | Notes |
|---|---|---|---|
| All WebSocket features (Upgrade, frames, Ping/Pong, 64KB cap, 1009, WS-over-H2, idle timeout) | **absent** | — | Entire WebSocket layer is deferred for v1; not in the required-tests list. |

### Request Smuggling / Security

| PROMPT row | Status | Evidence | Notes |
|---|---|---|---|
| CL-TE smuggling rejection | **present** | `crates/lb-security/src/smuggle.rs::check_cl_te`; `test_smuggling_cl_te_rejected` [required] | |
| TE-CL smuggling rejection | **present** | `SmuggleDetector::check_te_cl`; `test_smuggling_te_cl_rejected` [required] | |
| H2 downgrade smuggling rejection | **present** | `test_smuggling_h2_downgrade_rejected` [required] | |
| Rapid reset / continuation flood / HPACK bomb / QPACK bomb | **present** | See HTTP/2 and HTTP/3 rows above. | |
| NACL (radix trie allow/deny) | **absent** | — | |
| Per-IP connection rate limit (LRU 100K) | **absent** | — | |
| TLS fingerprinting (JA3) | **absent** | — | |

### Compression

| PROMPT row | Status | Evidence | Notes |
|---|---|---|---|
| zstd roundtrip | **present** | `crates/lb-compression/src/lib.rs`; `test_compression_zstd_roundtrip` [required] | |
| brotli roundtrip | **present** | `test_compression_brotli_roundtrip` [required] | |
| gzip roundtrip | **present** | `test_compression_gzip_roundtrip` [required] | |
| deflate roundtrip | **present** | `test_compression_deflate_roundtrip` [required] | |
| Transcode between algorithms | **present** | `test_compression_transcode_gzip_to_zstd` [required] | |
| Decompression-bomb cap | **present** | `test_compression_bomb_cap_fires` [required] | |
| BREACH posture / `BreachGuard` | **present** | `test_compression_breach_posture_no_leak` [required] | |
| Accept-Encoding q-value negotiation | **present** | `crates/lb-compression/src/lib.rs::negotiate` | No required integration test, but unit-tested inside the crate. |
| MIME whitelist / size threshold | **partial** | Size threshold exposed as a parameter; MIME whitelist is caller-enforced | |

### Proxy Protocol v1/v2

| PROMPT row | Status | Evidence | Notes |
|---|---|---|---|
| All PROXY protocol items | **absent** | — | Not yet implemented. Not in required-tests. |

### Connection / Node Management

| PROMPT row | Status | Evidence | Notes |
|---|---|---|---|
| H1 / H2 / QUIC / TCP pools | **absent** | — | |
| Shared eviction executor | **absent** | — | |
| Per-node O(1) connection count | **partial** | Gauge `AtomicU64` per listener in `ListenerState` | Per-backend accounting not yet split out. |
| TOCTOU-safe admission | **absent** | — | |
| Node state machine (ONLINE / OFFLINE / IDLE / MANUAL_OFFLINE) | **absent** | — | `BackendHealth` has Healthy / Unhealthy / Unknown only. |
| Bytes-sent / bytes-received tracking | **absent** | — | |
| H2 GOAWAY 5-s grace drain | **absent** | — | |

### Observability

| PROMPT row | Status | Evidence | Notes |
|---|---|---|---|
| Prometheus `/metrics` export | **partial** | `crates/lb-observability/src/lib.rs::MetricsRegistry` is the counter substrate | No HTTP endpoint wired. |
| JSON structured access logs | **partial** | `tracing-subscriber` JSON formatter is registered in `lb/src/main.rs` | Dedicated access-log format not yet modelled. |
| Per-request latency / XDP per-CPU stats / backend gauges | **absent** | — | |
| Control-plane push metrics | **absent** | — | |

### Configuration & Control Plane

| PROMPT row | Status | Evidence | Notes |
|---|---|---|---|
| TOML file configuration | **present** | `crates/lb-config/src/lib.rs`; `lb/src/main.rs` reads TOML | |
| Hot-reload via inotify + SIGHUP | **partial** | `ConfigManager` supports atomic reload; `test_controlplane_standalone_sighup_reload` [required] exercises the reload path. | inotify watcher is not yet registered in the binary; user must re-exec with SIGHUP. |
| Atomic config swap + validation | **present** | `ArcSwap` + `ConfigError::Validation`; `test_controlplane_rollback_on_invalid` [required] | |
| REST API (15+ CRUD endpoints) | **absent** | — | |
| gRPC services (NodeRegistration, ConfigDistribution, NodeControl) | **absent** | — | |
| Session token authentication | **absent** | — | |
| Heartbeat tracking | **absent** | — | |
| HA polling | **present** | `test_controlplane_ha_polling` [required] | |
| Delta sync / reconnect storm protection | **absent** | — | |

### Bootstrap & Lifecycle

| PROMPT row | Status | Evidence | Notes |
|---|---|---|---|
| Deterministic startup sequence | **present** | `lb/src/main.rs::async_main` | |
| Graceful shutdown (SIGTERM/SIGINT) | **partial** | `tokio::signal::ctrl_c` handled | SIGTERM path not yet distinguished; XDP detach path is a no-op (eBPF stubbed). |
| Hot config reload (SIGHUP) | **partial** | `test_controlplane_standalone_sighup_reload` at the library level | Binary wiring pending. |
| XDP attach / detach lifecycle | **deferred-with-rationale** | `crates/lb-l4-xdp/ebpf/src/main.rs` stub | |
| Connection drain on shutdown | **partial** | `test_reload_zero_drop_under_load` exercises the no-drop invariant during reload | Explicit drain-on-shutdown timer not yet wired. |

## Known-simulated areas

Three subsystems are deliberately present as userspace simulations,
with the simulation clearly documented in the crate docstring itself:

1. **`lb-l4-xdp`** — the XDP/eBPF L4 data plane. The production eBPF
   program will be compiled separately from the userspace workspace
   using aya-bpf (see ADR-0004 and ADR-0005). The simulation owns a
   HashMap-based conntrack table with FIFO eviction and a prime-sized
   Maglev table; the three `l4_xdp_*` integration tests exercise the
   exact semantics the real eBPF program must preserve. What is
   simulated: the BPF map schema, Maglev lookup, conntrack pinning,
   and hot-swap behaviour across backend-set churn. What is needed to
   deploy: fix the kernel / NIC target, finalise the aya-bpf object
   build, add a `CAP_BPF` + `CAP_NET_ADMIN` loader in `lb/src/main.rs`,
   and add an end-to-end test harness (network namespace +
   veth pair is the usual approach).

2. **`lb-quic`** — the QUIC transport layer. The simulation models
   datagram and stream forwarding with the same validation invariants
   the real `quiche`-backed code will need. What is simulated: the
   `QuicDatagram` / `QuicStream` shape, payload validation, connection
   ID handling. What is needed to deploy: adopt `quiche` per
   ADR-0003, build a real UDP listener, and carry through 0-RTT token
   validation (the underlying replay guard logic is already production
   in `lb-security::ZeroRttReplayGuard`).

3. **`lb-io`** — the I/O backend abstraction. `detect_backend()`
   always returns `Epoll` today. The `IoUring` variant is present in
   the enum so call sites compile against both, but the io_uring
   runtime probe is not implemented. ADR-0001 covers the plan.

## Deferred items

These items are explicitly out of scope for v1. The halting-gate
manifest does not list any test for them, so they do not block a
green gate; they are recorded here for honesty.

- **File-descriptor passing graceful upgrade.** Zero-downtime binary
  upgrades across two lb processes via `SCM_RIGHTS` fd-passing is
  listed in PROMPT.md and in the README, but the binary currently
  performs config reload only. ADR-0009 captures the intended
  mechanism. The `test_reload_zero_drop_under_load` integration
  test proves the config-reload path drops zero connections; the
  binary-upgrade path is a superset of that.

- **Real eBPF object build + attach.** See "Known-simulated areas"
  above.

- **Full h2spec, autobahn, testssl, and coniigit conformance
  harnesses in CI.** The conformance test files
  (`tests/conformance_h1.rs`, `_h2.rs`, `_h3.rs`) exist and pass as
  codec-level unit tests, but full external-harness runs are
  expected to live in a separate CI job against a running binary.

- **`cargo-fuzz` corpora and `fuzz/` crate.** No `fuzz/` directory
  exists in the repository. The halting gate does not require one.
  The pure-function framing layout of `lb-h1`, `lb-h2`, and
  `lb-h3` is written to be fuzz-ready; adding `fuzz/` is a
  post-v1 task.

- **Control-plane REST + gRPC API surfaces.** The control-plane
  trait seam and file-backed backend are present and tested; the
  full 15+ REST endpoints and three gRPC services from PROMPT.md
  §28 Control Plane section are deferred until the distributed
  backend is actually being built out.

- **WebSocket.** The entire WebSocket L7 layer is deferred; no
  required test in `manifest/required-tests.txt` references it.

- **PROXY protocol v1/v2.** Deferred; no required test.

- **TLS termination.** `rustls` is not yet wired into the binary.
  The ALPN matching in `lb-l7` is ready to receive it. Deferred
  because TLS requires certificate plumbing, hot-reload, OCSP
  stapling, and SNI routing — a large feature-block better done
  together with the cert-management control-plane API.

## Risk assessment per gap

The risk table below summarises the consequences of the gaps above
if the binary were deployed today.

| Gap | Production risk | Mitigation path |
|---|---|---|
| TLS absent | High — cleartext only on a general-purpose LB | Ship rustls integration and cert hot-reload before any internet exposure. |
| eBPF stub | Medium — L4 fast path unavailable; userspace proxy handles all traffic | Acceptable for internal / sub-10 Gbps deployments. Full XDP attach is the v2 performance story. |
| Connection pooling absent | Medium — per-request TCP dial increases upstream connection count and tail latency | Add pool per ADR (not written; follows ADR-0009 pattern). |
| No Prometheus HTTP endpoint | Medium — metrics collected but not scrapable | Small wiring change; `MetricsRegistry` already exposes the counters. |
| No rate limiting / NACL | Medium — no L4/L7 DoS shaping in-process | Partly compensated by the slowloris/slow-POST/smuggling detectors and by expecting a kernel-level or upstream rate limiter. |
| No WebSocket | Low — blocks WS customers but is clearly out of scope | Add as a dedicated feature block. |
| Quic simulation | Medium — real H3 clients cannot speak to the binary | Adopt `quiche`. All user-facing H3 framing and QPACK are production-ready. |
| No real SIGHUP watcher in binary | Low — user can re-exec; integration test covers library layer | Wire signal handler into `lb/src/main.rs`. |

The risks cluster around "not yet wired from the crate APIs into the
`lb` binary" rather than "the crate is wrong". The crate-level
invariants are covered by 296 tests, and every listed required test
is passing.

## Sources

- `PROMPT.md` §28 (Reference: Complete Feature Parity Checklist).
- `manifest/required-tests.txt` (the 59 blocking tests).
- `manifest/required-artifacts.txt` (closed artifact list).
- `scripts/halting-gate.sh` (mechanical definition of done).
- The code surface under `crates/lb-*/src/**/*.rs` and the tests
  under `tests/*.rs` listed per row above.
- Companion ADRs: `docs/decisions/ADR-0001..0010.md`.
- `docs/architecture.md` for the intended-to-be design.
