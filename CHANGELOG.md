# Changelog

All notable changes to ExpressGateway. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning aims for [SemVer](https://semver.org/spec/v2.0.0.html) once the first release is cut.

## [Unreleased] — Round-4 production-readiness drive

This section aggregates the Round-4 audit-driven work landing on
`prod-readiness/round-4`. Commits not yet on `main`.

### Lifecycle

- **CODE-2-03** (`fc050b0`) — `lb_core::Shutdown` graceful-drain
  primitive (`TaskTracker` + `CancellationToken`). `crates/lb/src/main.rs`
  installs SIGTERM/SIGINT/SIGUSR1 handlers; the drain sequence is
  `set_draining() → settle → cancel → listener abort → bounded wait →
  abort survivors`. `shutdown_aborted_connections_total` counter is
  emitted on deadline overflow.
- **CODE-2-02** — `std::panic::set_hook` installed before any spawn;
  emits a structured `tracing::error!(panic=true, location, backtrace)`
  and bumps the registry-backed `panic_total` counter. Pre-registry
  panics are buffered in an `AtomicU64` and drained on registry bind.
- **REL-2-02 / REL-2-04 / REL-2-06 / REL-2-08 / REL-2-13** — wave-2
  fix batch sequenced behind CODE-2-03; see `audit/reliability/round-2-review.md`.

### Protocol

- **CF-S22-QUICHE-H3-MIGRATION (S23–S26)** — the HTTP/3 termination front
  (server ingress+egress + the H3 upstream client) now rides
  `quiche::h3::Connection`; the hand-rolled H3 frame/QPACK layer is removed
  from production and the `lb-h3` crate deleted (wire-tests use a TEST-ONLY
  `lb-h3-testcodec`). A fresh `h3spec` run proves **7 of 9 carried conformance
  findings closed by construction** (control-stream state machine + critical-
  stream + CANCEL_PUSH; #16–21, #24) with **zero regressions**; QPACK header
  blocks now **Huffman-encode**. **Known quiche-0.28 limitations** (documented,
  not gateway bugs; close on a quiche upgrade — CF-QUICHE-UPGRADE): the
  transport-layer #1–10 deviations **plus** QPACK encoder/decoder uni-stream
  validation **#23/#25** (quiche reads-and-discards those instructions; inert
  under the gateway's static-only QPACK — no dynamic table is ever allocated)
  **plus** the §7.1 no-content-length truncation gap (CF-QUICHE-FRAME-
  COMPLETENESS). Validated: `--workspace --all-features` ×3 = 1442/0,
  clippy/fmt clean, R8 bounded-relay + F-MD-4 reset-mapping re-confirmed, a new
  H3-terminate soak BOUNDED over 960s (panic=0). See `audit/h3spec/s26-report.md`.
- **PROTO-2-01** (`132fc72`) — H2 requests with `:authority` ≠ `Host`
  are rejected (RFC 9113 §8.3.1).
- **PROTO-2-06 / PROTO-2-13** (`de5a93c`) — codec rename + `h2spec`
  skeleton + CONNECT-PROTOCOL test.
- **PROTO-2-07** (`2d33c5a`) — `StrippedRequest<B>` newtype gives a
  compile-time guarantee that hop-by-hop headers were filtered.
- **PROTO-2-08** (`e0c0daf`) — `HOP_BY_HOP` set matches RFC 9110
  §7.6.1 exactly.
- **PROTO-2-10** (`a70588e`) — smuggle-defence matrix + proof tests.
- **PROTO-2-11** (`deb9267`) — H3 actor emits `CONNECTION_CLOSE`
  with `H3_NO_ERROR = 0x0100` (RFC 9114 §8.1) on cancel.
  `graceful_h3_shutdown` is `pub` for cross-crate use.
- **PROTO-2-12** (`0d3f901`) — trailer pass-through baseline test
  pinned.
- **PROTO-2-14** (`e6a1cb1`) — `[runtime].tls13_only` knob.
- **PROTO-2-15** (`4ee05e0`) — SNI / `:authority` validator (wiring
  deferred to Wave 2c).
- **ROUND8-L7-01** — **behaviour change (operator-visible).** The
  client-visible `101 Switching Protocols` on a WebSocket upgrade is
  now emitted **only after** the upstream WS handshake has completed
  successfully (Pingora GHSA-xq2h-p299-vjwv / Envoy
  GHSA-rj35-4m94-77jh class, both CVSS 9.3). Previously the proxy
  returned `101` synchronously and dialed the upstream in a detached
  task. Consequences for operators:
  - WS upgrades now carry **one extra upstream-RTT of latency**
    (unavoidable; every reference proxy does this).
  - A failed upstream WS handshake now produces a clean
    **`502 Bad Gateway`** (upstream refused / unreachable) or
    **`504 Gateway Timeout`** (upstream dial / handshake budget,
    bounded by `[runtime].*` header timeout) **instead of**
    `101`-then-silent-close. External WS clients that previously
    relied on the buggy `101` (and only later noticed no traffic)
    will now see the failure immediately.
  - Bytes pipelined after the upgrade request are no longer admitted
    to an unread upgraded byte-stream on upstream failure
    (request-smuggling primitive closed).
- **ROUND8-OPS-06 / REL-2-07** — the W3C trace-context propagation
  library (`lb_observability::tracing_propagation`, shipped library-
  only in `1d462c7`) now has its first production L7 callsite. The
  H1/H2 proxies open a per-request span (`lb.l7.request`, carrying
  `trace_id` / `parent_id` / `http.method` / `http.target` /
  `http.status_code`) and inject a refreshed child `traceparent`
  onto the upstream request — including the WebSocket-upgrade dial —
  so an on-call engineer can pivot from an upgrade failure to the
  exact upstream dial.

### Security

- **SEC-2-01 / CODE-2-01** (`e00e85a`, `dc02517`, `5e7938f`) —
  `SmuggleDetector` and `SlowlorisWatchdog` wired into the H1 / H2
  hot path; `SecurityHooks` trait shim.
- **SEC-2-11** (`e44117d`) — XDP cap probe falls back to
  `CAP_SYS_ADMIN` on pre-5.8 kernels.
- **SEC-2-12** (`5064a11`) — userspace loader asserts `.license == "GPL"`
  on the BPF ELF at startup.

### Documentation

- **REL-2-01** (this commit) — RUNBOOK / DEPLOYMENT / METRICS / README
  rewrite. Every doc claim audited against current source:
  - README compression-feature line removed (the compression crate was deleted
    by L-001). <!-- doc-lint-allow -->
  - README panic-free claim verified by a non-test grep: zero hits
    across `crates/`.
  - DEPLOYMENT binary name corrected to `expressgateway`
    (`/usr/local/bin/expressgateway`, `target/release/expressgateway`).
  - DEPLOYMENT capability matrix added: `CAP_BPF || CAP_SYS_ADMIN`
    + `CAP_NET_ADMIN` depending on kernel.
  - RUNBOOK rewritten to cover every alert that can fire: `LbPanic`,
    `LbShutdownAborted`, `LbAcceptSaturation`, `LbAcceptErrors`,
    `LbAcceptShed`, `LbXdpConntrackFull`, `LbXdpAttachMode`,
    `LbXdpSamplerErrors`, `LbCertRotationFailed`, `LbReqDuration`,
    `LbReq5xx`, `LbDnsCacheMiss`, `LbPoolProbeFailures`,
    `LbConnectionsInflight`. Drain sequence and per-protocol drain
    signal matrix added.
  - METRICS enumerates every Prometheus family the binary will emit
    when fully wired, plus cardinality budget per family.
- **REL-2-02 drain integration test** (REL-2-02 commit) — multi-protocol
  drain test cases under `tests/reload_zero_drop.rs`:
  `test_sigterm_drains_h2_with_goaway`,
  `test_sigterm_drains_h1_with_connection_close`,
  `test_sigterm_drains_h3_with_connection_close`. `#[ignore]`'d
  until the listener-level `lb_core::Shutdown::token()` is plumbed
  through each protocol's `serve_connection` path.
- `scripts/ci/doc-lint.sh` — greps for stale references (see the
  script for the exact pattern list) and fails on hit. Wired into
  `.github/workflows/ci.yml`. <!-- doc-lint-allow -->

## [Unreleased] — Phase B production-readiness drive

This section aggregates the Phase B drive's commits since `b9853178` ("ExpressGateway: high-performance L4/L7 load balancer in Rust"). Commits are on `main`.

### Runtime & I/O (`lb-io`)

- `2bda92b9` — Real `io_uring` probe via NOP roundtrip; `Runtime` struct with 32 KB/64 KB watermarks matching PROMPT.md §7; per-socket option helpers for listeners, backends, and UDP; `io-uring = "0.7"` added to workspace deps. ADR-0001 updated `simulation → realised`.
- `631caff5` — `Runtime::listener_socket` / `Runtime::connect_socket` factory methods that bind + apply `ListenerSockOpts` / `BackendSockOpts` and return blocking std sockets ready for `tokio::net::X::from_std`.
- `9c37a740` — Single-shot `io_uring` op wrappers for `ACCEPT`, `RECV`, `SEND`, `SPLICE`; root binary (`crates/lb/src/main.rs`) now constructs `Runtime::new()` at startup and routes both listener bind and backend connect through `lb-io`.
- `edd11f02` — `TcpPool` with per-peer `DashMap<SocketAddr, Mutex<VecDeque<IdleConn>>>`, `PoolConfig` defaults from PROMPT.md §21, Pingora EC-01 non-blocking read-zero liveness probe before reuse, bounds enforced on return (`per_peer_max`, `total_max`), `max_age` + `idle_timeout` discarded on acquire.
- `2f4d136c` — `DnsResolver` with singleflight via `Arc<tokio::sync::OnceCell<_>>`, positive-TTL cap (300 s), negative-TTL cache (5 s) for NXDOMAIN, optional background refresh task; root binary uses it to resolve backend hostnames at startup.

### QUIC (`lb-quic`)

- `a50b6a81` — Simulation replaced with a real `quinn = "0.11"` endpoint over `rustls = "0.23"` (ring backend). `QuicEndpoint::server_on_loopback` + `client_on_loopback` for tests. `tests/quic_native.rs` now exercises a real UDP + TLS 1.3 handshake on 127.0.0.1 via rcgen-generated certs; the two manifest-locked test names (`test_quic_datagram_forwarding`, `test_quic_stream_forwarding`) are preserved. ADR-0003 rewritten `simulation → quinn`. `deny.toml` + `.cargo/audit.toml` ignore `RUSTSEC-2026-0009` with justification (reachable vector = nil; fix requires MSRV 1.88).

### XDP (`lb-l4-xdp`)

- `35491253` — `crates/lb-l4-xdp/ebpf/src/main.rs` replaced from a two-line stub with a real aya-ebpf XDP program: bounded Ethernet → IPv4 → TCP/UDP parsing, `#[map]`-declared `CONNTRACK` / `L7_PORTS` / `ACL_DENY` / `STATS`, XDP_PASS / XDP_DROP action selection. Standalone `lb-xdp-ebpf` crate (outside workspace members) with pinned nightly toolchain. `crates/lb-l4-xdp/src/loader.rs` adds an aya userspace loader (`load_from_bytes`, `kernel_load`, `attach`, `XdpMode::{Skb,Drv,Hw}`). `scripts/build-xdp.sh` is best-effort; the BPF ELF was not produced in this drive because `bpf-linker`'s transitive deps require rustc ≥ 1.88 and MSRV is 1.85. ADR-0004 + ADR-0005 rewritten to reflect real-program-plus-simulation.

### HTTP security (`lb-h2`, `lb-h1`)

- `a009a778` — Three additional Pingora-style flood detectors in `lb-h2`: `SettingsFloodDetector` (100 / 10 s), `PingFloodDetector` (50 / 10 s), `ZeroWindowStallDetector` (30 s). Integer-only rolling windows, same shape as the existing `RapidResetDetector` / `ContinuationFloodDetector` / `HpackBombDetector`. `lb-h1::parse::MAX_HEADER_BYTES = 65_536` with `parse_headers_with_limit` / `parse_trailers_with_limit` entries; oversize input returns `H1Error::HeadersTooLarge { limit, observed }`.

### TLS (`lb-security`)

- `a32e093b`, `154566b3` — `TicketRotator` in `crates/lb-security/src/ticket.rs`: daily rotation (configurable interval) with an overlap window where both current and previous keys decrypt. `RotatingTicketer` impls `rustls::server::ProducesTickets`. Wiring into a live TLS listener is deferred to the Pillar 3b follow-up.

### Operational

- `de5c6dbf` — `.cargo/audit.toml` mirrors `deny.toml`'s ignores so `cargo audit` and `cargo deny check` agree.
- `8db5ffef` — `docs/conformance/coverage.md` captures `cargo llvm-cov` output with an honest per-file gap table against FINAL_REVIEW §9.2's 80 % target.

### Documentation (Phase A.0)

- `50879461` — 18 research docs under `docs/research/` (RFC 9112/9113/9114/9000, HPACK+QPACK, gRPC, compression RFCs, DoS catalog, tokio+io_uring, aya eBPF, cross-cutting; reference-system studies for Pingora, Katran, Envoy, NGINX, HAProxy, Cloudflare LB, AWS ALB/NLB). 10 ADRs (`ADR-0001` through `ADR-0010`). Project-level docs: `architecture.md`, `gap-analysis.md`, `FINAL_REPORT.md`.

### Deferred to future pillars

See `docs/gap-analysis.md` for the full list. Summary:

- **Pillar 1c**: fixed fds (`IORING_REGISTER_FILES`), registered buffer pools (`IORING_REGISTER_BUFFERS`), tokio-reactor integration of io_uring ops, pipe-backed `SPLICE` for true zero-copy proxying.
- **Pillar 2b**: H2 upstream pool (needs lb-h2 client surface first), TcpPool re-keying on hostname changes via DNS refresh.
- **Pillar 3b**: stateless retry with token validation, 0-RTT replay protection, connection migration test, `h3` / `h3-quinn` deps + HTTP/3 server in the binary, Alt-Svc RFC 7838 injection, QUIC CID-routed connection pool, MSRV bump to 1.88 (drops RUSTSEC-2026-0009 ignore), wiring `TicketRotator` into a live TLS listener.
- **Pillar 4b**: XDP_TX rewrite with RFC 1624 incremental checksum, VLAN/IPv6 parsing, `LpmTrie` upgrade from HashMap<u32,u32> for ACLs, multi-kernel verifier matrix via xtask, CAP_BPF-gated integration test.

## Commit anchor

`b9853178` — ExpressGateway: high-performance L4/L7 load balancer in Rust (initial Rust port baseline).
