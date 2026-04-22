# Changelog

All notable changes to ExpressGateway. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning aims for [SemVer](https://semver.org/spec/v2.0.0.html) once the first release is cut.

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
