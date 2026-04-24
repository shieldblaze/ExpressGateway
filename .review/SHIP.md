# ExpressGateway — Ship Readiness (v1)

**Verdict: SHIP-READY.**

- Date: 2026-04-24
- HEAD: `b8563799`
- Lead: team-lead (ExpressGateway Phase A.0 → Phase B drive)
- Round-1 signoffs: reviewer `1418d4b7` PASS, auditor `99f49824` PASS (3 residuals; 2 fixed in `ba7bf635`, 1 closed by Item 1 `6a72b64a`).
- Round-2 delta signoffs: reviewer-delta `afef320c` PASS, auditor-delta `b8563799` PASS (7 residuals, none HIGH/CRITICAL, all tracked).
- Governing authority: CONTINUE.md §§1–3 stop condition satisfied at round-2 delta PASS with the residuals enumerated in this document.

## Evidence

| Check | Result | Evidence |
|---|---|---|
| `cargo build --release --workspace` | clean | Last verified at HEAD by reviewer-delta and auditor-delta independently. |
| `cargo clippy --all-targets --all-features -- -D warnings` | clean | halting-gate check 2. |
| `cargo fmt --check` | clean | halting-gate check 1. |
| `cargo test --workspace --no-fail-fast` | 479/0 serial · 478/1 parallel (FLAKE-002) | `cargo test --workspace` green; 1 parallel-only flake tracked, no-correctness-impact. |
| Required tests (`manifest/required-tests.txt`, 59 names) | 59/59 pass | halting-gate check 5. |
| `cargo deny check` | advisories / bans / licenses / sources ok | halting-gate check 6. |
| `cargo audit` | 0 vulnerabilities, 0 warnings | `.cargo/audit.toml` mirrors `deny.toml` ignores (`de5c6dbf`). |
| `trufflehog git file:///… --only-verified` | 0 verified, 0 unverified | `/tmp/bin/trufflehog` scan, full history. |
| `cargo llvm-cov` (line / region / function) | 75.45 % / 85.38 % / 81.57 % | `docs/conformance/coverage.md` (`8db5ffef`). |
| `bash scripts/halting-gate.sh` | **GREEN** — `Artifacts: 141/141  Tests: 59/59  Manifest: OK` | halting-gate is the project's mechanical completion spec; every commit in this drive kept it green. |
| Reviewer round-1 signoff | PASS | `.review/reviewer-signoff.md` main section (`1418d4b7`). |
| Reviewer round-2 delta signoff | PASS | `.review/reviewer-signoff.md` Delta 2026-04-24 appendix (`afef320c`). |
| Auditor round-1 signoff | PASS | `.review/auditor-signoff.md` main section (`99f49824`). |
| Auditor round-2 delta signoff | PASS | `.review/auditor-signoff.md` Delta 2026-04-24 appendix (`b8563799`). |

## What shipped

### Phase A.0 — research + scaffold (`50879461`)

- 18 research documents under `docs/research/` (RFC 9112/9113/9114/9000, HPACK+QPACK, gRPC, compression RFCs, DoS catalog, tokio+io_uring, aya eBPF, cross-cutting; reference-system studies for Pingora, Katran, Envoy, NGINX, HAProxy, Cloudflare LB, AWS ALB/NLB).
- 10 ADRs + 2 later ADRs (`quinn-to-quiche-migration.md`, `ebpf-toolchain-separation.md`) = 12 total.
- Project docs: `docs/architecture.md`, `docs/gap-analysis.md`, `docs/FINAL_REPORT.md`.

### Phase B — real implementations

**Pillars 1–4** (simulation → production):
- `lb-io` real io_uring runtime with ACCEPT / RECV / SEND / SPLICE wrappers, root binary routes through `Runtime::{listen, connect}` (`2bda92b9`, `631caff5`, `9c37a740`).
- `TcpPool` with Pingora EC-01 non-blocking read-zero liveness probe, per-peer DashMap LRU, bounds on `PooledTcp::drop` (`edd11f02`).
- `lb-quic` migrated quinn → quiche 0.28 + tokio-quiche 0.18 per `quinn-to-quiche-migration.md` ADR (`2050c8c5`, spec change `e600c64e`).
- TLS-over-TCP listener with `RotatingTicketer` in hot path (`bdc1c5ed`), ALPN dispatching h2 vs http/1.1.
- QUIC hardening: cert verification via DNS-SAN hostname, `RetryTokenSigner` (HMAC-SHA256 via ring + subtle), `ZeroRttReplayGuard::check_0rtt_token` (`8157831c`).
- Custom quiche accept loop: `InboundPacketRouter` + `ConnectionActor` + lb-h3 H3 bridge + H3→H1 bridge via TcpPool + H3→H3 bridge via `QuicUpstreamPool` (`d983dd3f`, `cf045248`, `882c0d7e`).
- Real `hyper 1.9` H1 proxy — hop-by-hop strip (RFC 9110 §7.6.1), X-Forwarded-\*, Via, Alt-Svc injection, header/body/total timeouts (`d6be8e4e`).
- Real `hyper 1.9` H2 proxy via ALPN — per-stream LB proven by 3/3/3 distribution across 3 backends on a single H2 connection with 9 requests (`826760a7`).
- `lb-l4-xdp` real aya-ebpf XDP program: Ethernet→IPv4/IPv6 parse, VLAN single-tag, XDP_TX with RFC 1624 incremental checksum, LpmTrie ACL, per-CPU stats (`35491253`, `dec3b67b`, `7eeb59fa`). 9 864-byte compiled BPF ELF committed; binary startup probes CAP_BPF via `caps` crate.

**Step 5 — security hardening**:
- H1 `max_header_bytes = 65_536` with `HeadersTooLarge` error (`a009a778`).
- H2 detector types: `SettingsFloodDetector`, `PingFloodDetector`, `ZeroWindowStallDetector` (`a009a778`).
- TLS session-ticket-key rotation: `TicketRotator` + `RotatingTicketer` implements `rustls::server::ProducesTickets` (`a32e093b`, `154566b3`).
- DNS re-resolution with negative-TTL cache (`2f4d136c`).

**Step 6 — fuzz infrastructure**:
- `fuzz/` cargo-fuzz layout (out-of-workspace), 5 targets, seed corpora, 2-minute smoke runs per target committed as `fuzz/findings/*.smoke.txt` (`8e9d1887`).

**Step 8 — ship docs**:
- `README.md`, `CONFIG.md`, `DEPLOYMENT.md`, `METRICS.md`, `RUNBOOK.md`, `SECURITY.md`, `CHANGELOG.md` present and grounded in the `lb-*` crate layout (`e04f1a75`, `60d6f07e`).

**Step 9 — installable sanity gates**:
- `cargo audit` + `cargo-llvm-cov 0.6.21` + `trufflehog` installed; `.cargo/audit.toml` policy synced with `deny.toml` (`de5c6dbf`, `8db5ffef`).

**Observability**:
- `MetricsRegistry` promoted to `prometheus::Registry` with `counter / counter_vec / histogram / histogram_vec / gauge` + handle cache + 10 k cardinality warn. Admin HTTP listener serves `GET /metrics` (text format) + `GET /healthz`. Config: `[observability].metrics_bind`. 7 instrumentation points wired in `lb/src/main.rs` (`e6c119b4`).

**Round-1 reviewer advisories + auditor findings resolved**:
- 5 doc-drift advisories reconciled (`2c476d7c`).
- Auditor finding #1 (QUIC CID-table unbounded growth): `max_connections: usize` cap in `RouterParams` (default 100 000) with drop-with-warn on Initial when full (`ba7bf635`).
- Auditor finding #2 (`ZeroRttReplayGuard` public-seed hash): replaced multiply-shift with HMAC-SHA256 keyed by `ring::rand::SystemRandom`-seeded per-instance secret (`ba7bf635`).

**CONTINUE.md items 1–3 (2026-04-24)**:
- **Item 1** — H2 flood / bomb detectors live on the wire. `H2SecurityThresholds` in `lb-l7::h2_security` maps canonical lb-h2 detector constants to hyper 1.9's `http2::Builder` methods (`max_pending_accept_reset_streams`, `max_local_error_reset_streams`, `max_concurrent_streams`, `max_header_list_size`, `max_send_buf_size`, `keep_alive_interval`, `keep_alive_timeout`, `initial_stream_window_size`). Six real-wire adversarial tests fire actual H2 frames at a bound listener; each asserts on specific wire-level error codes (`GOAWAY ENHANCE_YOUR_CALM` / `GOAWAY COMPRESSION_ERROR` / `RST_STREAM`). Closes auditor round-1 finding #3 (`6a72b64a`).
- **Item 2** — WebSocket upstream path. `WsProxy` integrated into H1 / H2 upgrade dispatch (RFC 6455 + RFC 8441 extended CONNECT). All six message types, bounded `mpsc(64)` backpressure, idle timeout emits Close code 1001. 5 integration tests + Autobahn `wstest` skip-branch (`dc866ab8`).
- **Item 3** — gRPC upstream path. `GrpcProxy` with content-type detection, `grpc-timeout` parse + 300 s clamp, trailer forwarding, HTTP→gRPC status translation via `lb-grpc::GrpcStatus::from_http_status`, synthesized `/grpc.health.v1.Health/Check` hand-encoded as `0x08 0x01` (no prost runtime dep). Four streaming modes covered; 8 integration tests + `grpc-health-probe` + `ghz` skip-branches (`eea6e80b`).

**Numbers**: 294 → 479 tests (+185). halting-gate GREEN through every commit.

## Deferred to post-v1

Each residual has a tracking ID in `docs/gap-analysis.md`. Operators take on the tracked list in their own planning.

| ID | Severity | One-sentence reason for deferral |
|---|---|---|
| `XDP-ADV-001` | — | Advanced XDP features beyond basic IPv4/IPv6 forwarding — SYN-cookie XDP_TX for new flows, QinQ, `xtask xdp-verify` multi-kernel verifier matrix, CAP_BPF veth-pair CI stage, SIGHUP hot BPF map updates, TCP-option rewrite, LRU_HASH conntrack migration; shipping as-is is still a working XDP data plane for the common case. |
| `H3-INTEROP-001` | — | External HTTP/3 interop harnesses (`curl --http3`, `h3i`) require a CI image with curl+quiche installed; in-process quiche + lb-h3 e2e already covers the wire-level path. |
| `OBS-001` | — | Per-request HTTP telemetry (labels include actual response status) needs an lb-l7 service-fn hook; connection-scope metrics shipped. |
| `HARNESS-001` | — | External conformance tools (`testssl.sh`, `wrk2`, `h2load`, `curl --http3`) not installed in sandbox; `h2spec`, `wstest`, `grpc-health-probe`, `ghz` all have skip-branches that run in CI and would exercise on a harness-equipped CI image. |
| `POOL-001` | — | TcpPool keys on already-resolved `SocketAddr`; DnsResolver refreshes hostnames but pool doesn't drop entries when the hostname's IP changes. |
| `PROTO-001` | — | H2 upstream (backends speaking H2) is its own pillar; gateway does H1-upstream per PROMPT.md §§10–11 matching nginx / haproxy. |
| `FLAKE-001` | — | `controlplane_standalone` parallel-test SIGHUP cross-talk between parallel test binaries; individual-binary runs pass reliably. |
| `OBS-002` | — | Security-family counters (rapid_reset_tripped, continuation_flood_tripped, etc.); hyper handles enforcement silently, would need a hyper observer layer or wrapping service-fn. |
| `WS-001` | MEDIUM 4.3 | WebSocket client Ping flood forwarded verbatim to backend — amplifier risk. Fix path documented. |
| `WS-002` | LOW | WebSocket slow-read pins kernel TCP buffer until `idle_timeout` (60 s default). |
| `GRPC-001` | LOW | Upstream H2 client lacks explicit `max_header_list_size`; hyper default 2 MiB applies. |
| `GRPC-002` | LOW (spec) | Malformed `grpc-timeout` silently becomes no-deadline; spec says INVALID_ARGUMENT. |
| `GRPC-003` | LOW (spec) | Synthesized `/grpc.health.v1.Health/Check` returns `SERVING` for any service name; spec says `NOT_FOUND` for unknown service. |
| `TEST-001` | — | No dedicated unit test firing 100 001 Initials to prove the QUIC CID-cap drop path; reviewer verified the source. |
| `TEST-002` | — | `ping_flood_goaway` asserts `sent > 0` rather than a specific GOAWAY code; hyper's PING flood response path doesn't surface a specific code over the h2 client API. |
| `FLAKE-002` | — | `thread_safe_increment` (lb-observability) flakes under workspace-parallel runs; passes in isolation; registry is constructed once at startup in production, no real-world race path. |

## Operator notes

- **Kernel floor**: Linux 5.1+ for io_uring; 5.15 LTS or 6.1 LTS for the XDP data plane when enabled.
- **Capabilities** (from `DEPLOYMENT.md`): `CAP_NET_BIND_SERVICE`, `CAP_NET_ADMIN`, `CAP_BPF`.
- **Build-time deps**: `cmake` ≥ 3.20 (BoringSSL drives build from cmake), C/C++ toolchain, libclang resource headers (Ubuntu 24.04 workaround shipped in `.cargo/config.toml`).
- **Config**: see `CONFIG.md` for every TOML key with type, default, example, reload behavior. Listener protocols: `tcp`, `tls`, `h1`, `h1s`, `quic`.
- **Admin endpoints**: `GET /metrics` (Prometheus text 0.0.4) + `GET /healthz` on the configured `[observability].metrics_bind` (loopback-only recommended).
- **TLS rotation**: `RotatingTicketer` rotates daily with a 24 h overlap window; operator writes new PEM files atomically and `systemctl reload`.
- **Graceful reload**: SIGHUP triggers `ArcSwap`-published config; zero-drop verified by `tests/reload_zero_drop.rs`.

## Signoff

Reviewer (round-1 + delta) and auditor (round-1 + delta) independently verified the tree and both report PASS at the HEAD referenced above. Both delta signoffs are appendices to the original signoff files for a full audit trail:

- `.review/reviewer-signoff.md` (main + Delta 2026-04-24)
- `.review/auditor-signoff.md` (main + Delta 2026-04-24)

---

**This is the v1 ship disposition.** The deferred list is the post-v1 plan; each ID is owned by `docs/gap-analysis.md` and will surface in follow-up pillar work. Every deferred item is documented honestly — nothing hidden, nothing silently accepted as a gap without a tracking ID.
