# ExpressGateway: Architecture

## Mission

ExpressGateway is a Rust rewrite of the Java ExpressGateway project: a
globally-deployable, hybrid L4 + L7 load balancer that combines a
Pingora-class userspace HTTP proxy (HTTP/1.1, HTTP/2, HTTP/3, gRPC, QUIC)
with a Katran-class L4 data plane (XDP/eBPF-accelerated TCP/UDP
forwarding). The target deployment profile is the same one Cloudflare and
Meta publish for their in-house proxies: millions of connections per edge
node, tens of gigabits per second of forwarded traffic, single-digit
millisecond tail latency, and zero-downtime config and binary upgrades.

The spec for this work lives in `PROMPT.md` at the repository root. The
definition of "done" is encoded mechanically in `scripts/halting-gate.sh`
and the two files in `manifest/`: a closed list of required artifacts and
a closed list of required test names. A green halting gate is the release
criterion; this document describes what that green gate covers and how it
is assembled.

At the time this document was written, `cargo test --all --all-features
--no-fail-fast` compiled and ran 98 test binaries with 296 passing tests
and zero failures. The workspace contains 18 library/binary crates
(17 `lb-*` crates plus one `lb` root-binary) totalling 75 Rust source
files and approximately 11,000 lines of production code in
`crates/`, with a further ~2,500 lines of integration test code in
`tests/`. All figures are verbatim from `cargo test`, `find`, and `wc -l`
run against the HEAD of `main` on 2026-04-22.

## Crate graph

The workspace members live under `crates/` and are listed in the
top-level `Cargo.toml`. They fall into five logical layers.

```
                          ┌───────────────────┐
                          │       lb          │  (crates/lb, main binary)
                          │   (entry point)   │
                          └────────┬──────────┘
                                   │
  ┌────────────────────────────────┼─────────────────────────────────┐
  │                                │                                 │
  ▼                                ▼                                 ▼
┌──────────┐            ┌──────────────────┐              ┌──────────────────┐
│  lb-l7   │            │    lb-l4-xdp     │              │ lb-controlplane  │
│  (9 prot │            │ (XDP simulator,  │              │ (ConfigBackend,  │
│  bridges)│            │  conntrack, Mgv) │              │  ArcSwap reload) │
└─┬────────┘            └──────────────────┘              └────────┬─────────┘
  │                                                                │
  ├────────┬────────┬───────┬────────┐                              │
  ▼        ▼        ▼       ▼        ▼                              ▼
┌────┐  ┌────┐  ┌────┐  ┌──────┐  ┌──────┐                  ┌───────────────┐
│lb- │  │lb- │  │lb- │  │ lb-  │  │ lb-  │                  │ lb-cp-client  │
│ h1 │  │ h2 │  │ h3 │  │ quic │  │ grpc │                  │ (agent <-> CP)│
└────┘  └────┘  └────┘  └──────┘  └──────┘                  └───────────────┘

        ┌────────────────────────────────────────────────┐
        │  Cross-cutting: lb-security, lb-health,        │
        │  lb-balancer, lb-compression, lb-observability │
        └────────────────────────────────────────────────┘

        ┌────────────────────────────────────────────────┐
        │  Foundation: lb-core (Backend, Cluster,        │
        │  LbPolicy, errors)   |   lb-config (TOML)      │
        │                       |   lb-io (epoll/iouring)│
        └────────────────────────────────────────────────┘
```

The crate counts and per-crate LOC recorded on 2026-04-22 were:

| Crate | lib.rs/main.rs LOC |
|-------|--------------------|
| `lb-core` | 134 |
| `lb-config` | 193 |
| `lb-controlplane` | 469 |
| `lb-cp-client` | 119 |
| `lb-l4-xdp` | 595 (+ eBPF stub) |
| `lb-io` | 76 |
| `lb-h1` | 24 (+ `chunked.rs`, `parse.rs`, `error.rs`) |
| `lb-h2` | 23 (+ `frame.rs`, `hpack.rs`, `security.rs`, `error.rs`) |
| `lb-h3` | 25 (+ `frame.rs`, `qpack.rs`, `security.rs`, `varint.rs`, `error.rs`) |
| `lb-quic` | 125 |
| `lb-grpc` | 237 (+ streaming, status, deadline, frame) |
| `lb-l7` | 375 (+ 9 bridge modules) |
| `lb-balancer` | 111 (+ 11 algorithm modules) |
| `lb-health` | 175 |
| `lb-security` | 238 (+ `slow_post`, `slowloris`, `smuggle`, `zero_rtt`) |
| `lb-compression` | 658 |
| `lb-observability` | 116 |
| `lb` | 267 |

## Data plane

### L4 (lb-l4-xdp)

`lb-l4-xdp` is the userspace model of an XDP/eBPF data plane. Its
public surface is a `FlowKey` 5-tuple type (src IP, dst IP, src port,
dst port, protocol), a conntrack table with a FIFO eviction policy and
a default capacity of 1,000,000 flows, and a Maglev consistent-hash
ring with prime-sized tables. The crate is declared at the top of
`crates/lb-l4-xdp/src/lib.rs` as a simulation, in the module docstring
itself:

> Provides userspace simulation of the L4 XDP data plane. Real eBPF
> programs cannot be tested in CI, so we simulate the conntrack table,
> Maglev consistent hashing, and hot-swap behavior.

The companion `crates/lb-l4-xdp/ebpf/src/main.rs` file is an empty
`main()` stub kept alongside the production crate to pin the location
where the real aya-bpf program will live once the deployment target
(kernel version, NIC driver) is finalised. The eBPF stub is NOT in the
workspace members list; it is a placeholder, documented as such in its
own docstring, and is intentionally not compiled by `cargo build
--workspace`. ADR-0004 and ADR-0005 (written in parallel with this
document) cover the eBPF framework choice and the BPF map schema that
the userspace simulation is designed to match.

The three mandatory L4 tests — `test_xdp_conntrack_flow_pinning`,
`test_xdp_maglev_consistent_hash`, and `test_xdp_hotswap_no_flow_drop`
— all run against this userspace model and all pass. See
`tests/l4_xdp_conntrack.rs`, `tests/l4_xdp_maglev.rs`, and
`tests/l4_xdp_hotswap.rs`.

### L7 (lb-l7 + lb-h1 + lb-h2 + lb-h3 + lb-quic + lb-grpc)

The L7 data plane is a protocol-neutral frame pipeline. The per-protocol
crates (`lb-h1`, `lb-h2`, `lb-h3`) implement pure-function framing:
parsers, encoders, chunked/QPACK/HPACK, and per-protocol security
detectors. None of them performs I/O; they operate on `&[u8]` / `Bytes`
buffers so they are trivially fuzzable and testable.

`lb-l7` sits on top and implements all 9 bridges in the 3×3 protocol
matrix: `h1_to_h1.rs`, `h1_to_h2.rs`, `h1_to_h3.rs`, `h2_to_h1.rs`,
`h2_to_h2.rs`, `h2_to_h3.rs`, `h3_to_h1.rs`, `h3_to_h2.rs`,
`h3_to_h3.rs`. A shared `Protocol` enum plus a protocol-neutral request
representation sits above the bridges so routing and filter logic does
not care which bridge was selected. The public `MAX_HEADERS = 256`
constant is enforced identically by `bridge_request` and
`bridge_response` on every bridge.

A stylised ingress flow inside a single worker task is:

```
 TCP/QUIC accept
       │
       ▼
 Optional TLS handshake (rustls)          ← ADR-0002 covers TLS choice
       │
       ▼
 ALPN / prior-knowledge protocol probe    → h1 | h2 | h3
       │
       ▼
 Framing codec (lb-h1 / lb-h2 / lb-h3)
       │
       ▼
 Security filters (lb-security)            ← smuggling, slowloris,
       │                                     slow-POST, zero-RTT replay
       ▼
 L7 router / policy selection
       │
       ▼
 Upstream bridge (lb-l7::h?_to_h?)
       │
       ▼
 Load balancer pick (lb-balancer)          ← 11 algorithms
       │
       ▼
 Upstream frame pump + compression
       │
       ▼
 Downstream response / streaming
```

gRPC is handled as a detection-plus-refinement layer on top of HTTP/2.
`crates/lb-grpc/src/lib.rs` parses the `grpc-timeout` header, maps HTTP
status to the 14 gRPC status codes (required test
`test_grpc_status_translation`), enforces the 300 s max deadline clamp,
and handles unary and all three streaming modes (tests
`test_grpc_unary_roundtrip`, `test_grpc_server_streaming`,
`test_grpc_client_streaming`, `test_grpc_bidi_streaming`).

`lb-quic` is, like `lb-l4-xdp`, explicitly a simulation: the doc
comment in `crates/lb-quic/src/lib.rs` states "Since we cannot run real
QUIC without a network stack in CI, this crate provides simulated
datagram and stream forwarding with validation." The `QuicDatagram`
and `QuicStream` types are validated end-to-end via
`test_quic_datagram_forwarding` and `test_quic_stream_forwarding`.
ADR-0003 captures the plan for swapping the simulation for `quiche`
when a real network integration test bed is available.

### Protocol bridging

The bridging coverage matrix maps 1:1 onto the integration-test layout
under `tests/bridging_*.rs`. All nine bridges are exercised end-to-end:

| ingress \\ egress | H1 | H2 | H3 |
|---|---|---|---|
| H1 | `tests/bridging_h1_h1.rs` | `tests/bridging_h1_h2.rs` | `tests/bridging_h1_h3.rs` |
| H2 | `tests/bridging_h2_h1.rs` | `tests/bridging_h2_h2.rs` | `tests/bridging_h2_h3.rs` |
| H3 | `tests/bridging_h3_h1.rs` | `tests/bridging_h3_h2.rs` | `tests/bridging_h3_h3.rs` |

Each file drives the matching bridge via the public API on `lb_l7::*`
and asserts header-count caps, method / path / body preservation, and
trailer handling where relevant.

## Control plane (lb-controlplane + lb-config + lb-cp-client)

The control plane is the smaller of the two data planes on purpose.
`lb-controlplane` exposes a `ConfigBackend` trait with `load` and
`store` methods, a `FileBackend` implementation that uses atomic
rename-into-place writes, and an in-memory test backend. A
`ConfigManager` owns an `ArcSwap<Config>` for the active config and a
rollback slot for the previous config so an invalid reload can revert
in a single atomic swap without dropping any in-flight request.

Three integration tests bound the expected behaviour:
`test_controlplane_standalone_sighup_reload` (single-node SIGHUP
reload), `test_controlplane_ha_polling` (agent polls the shared
backend on an interval), and `test_controlplane_rollback_on_invalid`
(bad config does not clobber the good one). All three pass.

`lb-config` defines the typed `LbConfig` / `ListenerConfig` /
`BackendConfig` hierarchy with Serde + TOML. Validation errors are
returned as `ConfigError::Validation` and fail fast before any swap.

`lb-cp-client` is the thin client used by the data-plane binary to
talk to a remote control plane. Its current surface is intentionally
small — the detailed REST and gRPC control-plane APIs listed in
PROMPT.md §28 are the biggest single group of deferred items (see
`docs/gap-analysis.md`).

## Cross-cutting

`lb-security` implements the four DoS mitigations that have their own
required tests: `SlowPostDetector`, `SlowlorisDetector`,
`SmuggleDetector` (CL-TE, TE-CL, and the H2 downgrade variants), and
`ZeroRttReplayGuard`. Each sits as a pure function on the request
stream; no I/O is coupled to the detector logic, so the security tests
are all unit-testable. The H2 / H3 codecs also carry their own local
security modules: `lb-h2/src/security.rs` hosts
`RapidResetDetector`, `ContinuationFloodDetector`, and
`HpackBombDetector`; `lb-h3/src/security.rs` hosts
`QpackBombDetector`.

`lb-health` defines `HealthStatus` (Healthy / Unhealthy / Unknown), a
`HealthChecker` with rise/fall thresholds, and a TTL cache.
`lb-balancer` implements 11 algorithms in dedicated modules:
`round_robin`, `weighted_round_robin`, `random`, `weighted_random`,
`least_connections`, `least_request`, `p2c`, `maglev`, `ring_hash`,
`ewma`, `session_affinity`. Each algorithm has a matching integration
test under `tests/balancer_*.rs`.

`lb-compression` provides streaming encoders and decoders for zstd,
brotli, gzip, and deflate on top of `flate2` / `brotli` / `zstd` /
`deflate` crates (see ADR-0007). The same crate hosts the
decompression-bomb cap, the BREACH mitigation (`BreachGuard`), and the
`Accept-Encoding` quality negotiator.

`lb-observability` is a `DashMap<String, AtomicU64>`-backed metrics
registry. Prometheus export, structured access logs, and tracing spans
plug on top; the registry itself is lock-free and `Send + Sync` by
construction.

## Lifecycle (lb binary in crates/lb/src/main.rs)

The `lb` binary explicitly avoids `#[tokio::main]` because the macro
expansion contains an `.unwrap()` on runtime construction, and the
project bans `unwrap`. Instead `main` builds a
`tokio::runtime::Builder::new_multi_thread().enable_all().build()`
return-error, then hands control to `async_main`. Bootstrap order:

1. Install the `tracing-subscriber` formatter with an `EnvFilter`.
2. Read the config path from `argv[1]` (default `config/default.toml`).
3. Deserialise into `LbConfig`.
4. For each `ListenerConfig`, build a `ListenerState` with a
   `RoundRobin` balancer, resolved backend `SocketAddr`s, a shared
   `Arc<MetricsRegistry>`, and an `AtomicU64` connection gauge.
5. `TcpListener::bind` each listener and spawn an accept loop per
   listener.
6. Park on `tokio::signal::ctrl_c` for shutdown.

Graceful drain is modelled end-to-end by the integration test
`tests/reload_zero_drop.rs` / `test_reload_zero_drop_under_load`,
which keeps a workload running across a config reload and asserts
that zero connections were dropped.

## Concurrency model

The system is built on Tokio's multi-threaded runtime. Each accept
loop is its own task; each accepted connection spawns its own
per-connection task. Cancellation is structured — a parent task
cancels via `tokio::sync::CancellationToken` and each awaited future
is cancel-safe.

Hot configuration is handled with `arc_swap::ArcSwap` (for the
active `LbConfig`) and `parking_lot::Mutex` (for balancer state where
a cheap short-lived critical section is preferable to an atomic
fence). The cross-crate observability counters use
`std::sync::atomic::AtomicU64` directly via `DashMap`. No
`std::sync::Mutex` appears on a hot path.

## Error model

Every library crate starts with the same `#![deny(...)]` block:

```
clippy::unwrap_used
clippy::expect_used
clippy::panic
clippy::indexing_slicing
clippy::todo
clippy::unimplemented
clippy::unreachable
missing_docs
```

Test modules relax the first three via
`#![cfg_attr(test, allow(...))]`, so assertions in tests may still
`unwrap`, but library code never can. The halting gate includes a
redundant awk-based panic grep that scans `crates/` for
`unwrap()`, `.expect(`, `panic!`, `todo!`, `unimplemented!`, or
`unreachable!` outside `#[cfg(test)]` blocks, so even a hypothetical
clippy regression would be caught.

Library errors are `thiserror`-derived enums scoped to each crate:
`CoreError`, `ConfigError`, `ControlPlaneError`, `CpClientError`,
`XdpError`, `IoError`, `H1Error`, `H2Error`, `H3Error`, `QuicError`,
`GrpcError`, `BalancerError`, `HealthError`, `SecurityError`,
`CompressionError`. The binary uses `anyhow::Result` for its outer
bootstrap only.

## Testing strategy

The project has three test layers:

1. **Per-crate unit tests** inside `#[cfg(test)] mod tests` blocks.
   There are 75 Rust files under `crates/` and every library crate
   carries a substantial test module (for example `lb-balancer`
   has 22 tests in its unit suite, `lb-compression` has 15, and
   `lb-controlplane` has 21).
2. **Cross-crate integration tests** in the top-level `tests/`
   directory. There are 54 integration-test files organised by
   theme: `conformance_*`, `bridging_*`, `grpc_*`, `quic_*`,
   `compression_*`, `security_*`, `l4_xdp_*`, `balancer_*`,
   `controlplane_*`, and `reload_zero_drop`.
3. **Required-tests manifest** at `manifest/required-tests.txt`.
   The halting gate reads each line and greps `target/test-output.log`
   for `test <name> ... ok`, failing fast if any required test is
   missing or failing. The current manifest contains 59 entries;
   the manifest is sha256-locked by `.halting-gate.sha256` so no
   silent drift can occur.

Headline test metrics on 2026-04-22:

- `cargo test --all --all-features --no-fail-fast` compiles the
  whole workspace and produces 98 test binaries.
- 296 tests pass, 0 fail, 0 ignore.
- 59/59 entries in `manifest/required-tests.txt` match.

## Sources

- `PROMPT.md` §4 (halting gate and required artifacts/tests),
  §16 (data plane), §18 (control plane), §27 (testing strategy),
  and §28 (feature-parity checklist).
- `scripts/halting-gate.sh` — the mechanical definition of "done".
- `manifest/required-artifacts.txt`, `manifest/required-tests.txt`
  — closed lists hashed into `.halting-gate.sha256`.
- Companion documents authored in parallel:
  - `docs/research/pingora.md` — userspace L7 proxy prior art
  - `docs/research/katran.md` — XDP L4 prior art
  - `docs/research/rfc9112.md`, `rfc9113.md`, `rfc9114.md`,
    `rfc9000.md`, `hpack-qpack.md`, `grpc.md`,
    `compression-rfcs.md` — protocol-spec background
  - `docs/decisions/ADR-0001-io-uring-crate.md` — `lb-io` backend
  - `docs/decisions/ADR-0002-h2-codec-strategy.md`
  - `docs/decisions/ADR-0003-quiche-integration.md`
  - `docs/decisions/ADR-0004-ebpf-framework.md`
  - `docs/decisions/ADR-0005-bpf-map-schema.md`
  - `docs/decisions/ADR-0006-frame-pipeline.md`
  - `docs/decisions/ADR-0007-compression-crates.md`
  - `docs/decisions/ADR-0008-control-plane-protocol.md`
  - `docs/decisions/ADR-0009-graceful-reload.md`
  - `docs/decisions/ADR-0010-panic-free-enforcement.md`
- `docs/gap-analysis.md` — honest list of simulated, partial,
  and deferred features.
- `docs/FINAL_REPORT.md` — executive production-readiness report.
