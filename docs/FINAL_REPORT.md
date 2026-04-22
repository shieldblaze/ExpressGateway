# ExpressGateway: Production Readiness Final Report

## Executive summary

ExpressGateway has reached the state of a mechanically-verifiable v1:
`scripts/halting-gate.sh` is a pure function of repo state, the
`manifest/` files are hash-locked into `.halting-gate.sha256`, and
every blocking check is either already green or blocked only on the
three documentation artifacts this report concludes (architecture,
gap analysis, final report). The data plane's codec and bridging
layers are production-grade and have full required-test coverage.
Two subsystems — L4-XDP and QUIC transport — are explicitly present
as userspace simulations with the same invariants the production
kernel-level implementations must preserve; these are deferred to
v2 with a documented rationale and are NOT counted as "done" in
this report.

| Dimension | Verdict | Evidence |
|-----------|---------|----------|
| Build | Green | `cargo build --workspace` implicit via `cargo test`; no errors. |
| Lint | Green | halting gate step 2 (`cargo clippy --all-targets --all-features -- -D warnings`). |
| Format | Green | halting gate step 1 (`cargo fmt --check`). |
| Tests | Green | 296 passing, 0 failing, 0 ignored across 98 binaries. 59/59 required tests match `manifest/required-tests.txt`. |
| Panic-free | Green | Halting-gate awk grep of `unwrap\|expect\|panic!\|todo!\|unimplemented!\|unreachable!` in non-test `crates/` code returns empty. Every library crate enforces the same `#![deny(...)]` block. |
| Dependency hygiene | Green | `cargo deny check` (halting-gate step 6) passes with the 8 vetted RUSTSEC advisories explicitly ignored in `deny.toml`. |
| Documentation | Green on this PR | All 141 required artifacts in `manifest/required-artifacts.txt` will be present once this file and the two sibling docs land. |
| Performance posture | Partial | Codec + bridge layers are allocation-tight and benchmarked (`bench/criterion/*.rs`); end-to-end benchmarks against a real load generator are deferred. |
| Security posture | Partial | DoS mitigations (smuggling, slowloris, slow-POST, rapid-reset, continuation flood, HPACK/QPACK bomb, 0-RTT replay, compression bomb, BREACH) are implemented and tested; TLS termination, NACL, and per-IP rate limits are deferred. |
| Deployability | Partial | Binary boots, reads TOML, accepts TCP, round-robins across backends, and handles Ctrl-C. Systemd integration, `CAP_BPF`/`CAP_NET_ADMIN` plumbing, and reload fd-passing are deferred (see "Recommended next actions"). |

## Scope

This project is a ground-up Rust rewrite of the Java ExpressGateway
project. The specification is `PROMPT.md` at the repository root
(2,329 lines; frozen). The completion criterion is mechanical:
`scripts/halting-gate.sh` exits 0. The halting gate performs seven
checks: toolchain+format, clippy with `-D warnings`, the Cloudflare-
2025-inspired panic grep, required-artifacts presence, required-tests
all-present-and-passing, `cargo deny check`, and
manifest-integrity-hash.

"Production-readiness" in this report means: "if the halting gate
is green, the codebase can be released as v1; the gaps below are
the honest v2 / post-v1 roadmap." We do **not** claim feature
parity with a mature commercial edge proxy. We claim verifiable
adherence to a closed, mechanically-checked specification.

## What was done

From `git log`, the main milestone landed in commit `b9853178`:
"ExpressGateway: high-performance L4/L7 load balancer in Rust".
The current artifacts produced at that milestone are:

- 18 workspace crates (17 `lb-*` libraries + `lb` binary):
  `lb-core`, `lb-config`, `lb-controlplane`, `lb-cp-client`,
  `lb-l4-xdp`, `lb-io`, `lb-h1`, `lb-h2`, `lb-h3`, `lb-quic`,
  `lb-grpc`, `lb-l7`, `lb-balancer`, `lb-health`, `lb-security`,
  `lb-compression`, `lb-observability`, `lb`.
- 75 Rust source files under `crates/`.
- 54 integration-test files under `tests/` (~2,500 test LOC).
- 5 criterion benchmarks under `bench/criterion/`.
- `manifest/required-artifacts.txt` (141 entries) and
  `manifest/required-tests.txt` (59 entries), both hash-locked.
- `scripts/halting-gate.sh` (the release gate).
- `deny.toml`, `rust-toolchain.toml` (pinned to 1.85 / edition
  2024), `Cargo.toml` workspace declaration, `LICENSE` (GPL-3.0).
- CI workflows: `.github/workflows/ci.yml` and `release.yml`.
- `docker/Dockerfile` and `docker/Dockerfile.test` for container
  packaging.

The three documentation files concluded by this PR
(`docs/architecture.md`, `docs/gap-analysis.md`, this file) plus
the sibling `docs/research/*.md` and `docs/decisions/ADR-*.md`
files written in parallel by other agents are the final artifacts
the halting gate requires.

## What works (evidence-backed)

Each claim below is verifiable by running the named command.

- **Workspace compiles cleanly.** `cargo test --all --all-features
  --no-fail-fast` builds every crate with no errors and produces
  98 test binaries.
- **296 tests pass, 0 fail, 0 ignore.** Summed from the `test
  result: ok. N passed; 0 failed; 0 ignored` lines of the same
  command.
- **Required-tests manifest is fully satisfied.** 59 entries in
  `manifest/required-tests.txt`; every one is greppable in
  `target/test-output.log` as `test <name> ... ok`.
- **No panics, unwraps, expects, todos, or unreachables in
  non-test library code.** Halting-gate step 3 (awk scanner over
  `crates/`) returns empty. Individually every crate's `src/lib.rs`
  starts with the same `#![deny(clippy::unwrap_used,
  clippy::expect_used, clippy::panic, clippy::indexing_slicing,
  clippy::todo, clippy::unimplemented, clippy::unreachable,
  missing_docs)]` block.
- **11 load-balancing algorithms implemented and tested.** See
  `crates/lb-balancer/src/{round_robin,weighted_round_robin,random,
  weighted_random,least_connections,least_request,p2c,maglev,
  ring_hash,ewma,session_affinity}.rs`; corresponding integration
  tests in `tests/balancer_*.rs`.
- **9-way HTTP protocol bridging.** `tests/bridging_h{1,2,3}_h{1,2,3}.rs`
  exercises the full 3×3 matrix via `lb_l7::*`.
- **Per-protocol framing with DoS mitigations.**
  `crates/lb-h1/src/parse.rs`, `chunked.rs`; `crates/lb-h2/src/frame.rs`,
  `hpack.rs`, `security.rs`; `crates/lb-h3/src/frame.rs`, `qpack.rs`,
  `security.rs`, `varint.rs`. Required tests:
  `test_h{1,2,3}_*`.
- **Security detectors for CL-TE, TE-CL, H2-downgrade, slowloris,
  slow-POST, 0-RTT replay, rapid reset, CONTINUATION flood,
  HPACK-bomb, QPACK-bomb.** See `crates/lb-security/src/*.rs` and
  per-protocol security modules.
- **gRPC proxy with unary + all three streaming modes, deadline
  parsing, 14-status-code translation.** `crates/lb-grpc/src/*.rs`
  and `tests/grpc_*.rs`.
- **Compression: zstd, brotli, gzip, deflate, transcode, bomb cap,
  BREACH posture, q-value negotiation.** `crates/lb-compression/src/lib.rs`
  (658 LOC) and `tests/compression_*.rs`.
- **Config hot-reload with atomic rollback.** `crates/lb-controlplane/src/lib.rs`
  (`FileBackend`, `ConfigManager`, `ArcSwap`) and
  `tests/controlplane_*.rs` (`standalone`, `ha`, `rollback`).
- **Zero-drop reload under load.** `tests/reload_zero_drop.rs`.
- **Binary boots, binds TCP listeners, round-robins across backends,
  and honours Ctrl-C.** `crates/lb/src/main.rs` (267 LOC).

## What is simulated

Three subsystems are explicitly and visibly simulated in-crate. Each
crate's module docstring states it is a simulation; the halting gate
is aware of this (the simulations are fully tested against their
interface invariants, which is the condition for the green gate).

1. **`lb-l4-xdp`** — Userspace simulation of the XDP/eBPF L4 data
   plane. `crates/lb-l4-xdp/ebpf/src/main.rs` is an empty stub; the
   production eBPF program needs to be built with aya-bpf and a
   pinned kernel target. See ADR-0004 (eBPF framework) and
   ADR-0005 (BPF map schema).
2. **`lb-quic`** — Userspace simulation of QUIC datagram/stream
   forwarding. The future production implementation adopts `quiche`
   per ADR-0003.
3. **`lb-io`** — `detect_backend()` always returns `Epoll`. The
   `IoUring` variant exists in the enum so downstream code compiles
   against both; the runtime probe for kernel io_uring support
   needs to be implemented per ADR-0001.

The other two "harness-level" gaps worth calling out explicitly in
this report:

- **No `fuzz/` directory.** `cargo-fuzz` corpora and fuzz targets
  are not checked in. The halting gate does not require them. The
  pure-function framing in `lb-h1`, `lb-h2`, and `lb-h3` was
  written fuzz-ready (no I/O, input as `&[u8]`), but the
  12 targets hinted at in the broader project README do not exist
  as code in this repo.
- **No h2spec / autobahn / testssl / grpc-interop harnesses in
  CI.** The `tests/conformance_h{1,2,3}.rs` files exist as
  codec-level unit tests; full external-harness runs against a
  live binary are deferred.

## Deferred work for post-v1

Sorted by risk-to-deployment (from `docs/gap-analysis.md`):

1. TLS termination with rustls (cert loading, SNI routing, hot-reload,
   ALPN wiring, OCSP stapling, mTLS modes).
2. Real eBPF program build + attach (aya-bpf, `CAP_BPF` loader,
   veth test bed).
3. Connection pooling for upstreams (H1 FIFO, H2 shared, QUIC CID,
   TCP validated-on-acquire).
4. Prometheus HTTP `/metrics` exporter (counters already exist in
   `lb-observability`).
5. NACL radix-trie allowlist/denylist + per-IP rate limiter.
6. PROXY protocol v1/v2 decode/encode.
7. WebSocket upgrade + frame proxying + WS-over-H2.
8. Control-plane REST + gRPC API surfaces.
9. Quiche-backed real QUIC transport.
10. `io_uring` runtime probe + backend.
11. FD-passing binary-upgrade path (config reload is already
    zero-drop; binary upgrade is a superset).
12. `cargo-fuzz` corpus and in-CI fuzz runs.

## Recommended next actions for deployment

A minimal viable production deployment plan:

1. **Systemd unit.** Create `/etc/systemd/system/expressgateway.service`:
   ```
   [Service]
   ExecStart=/usr/local/bin/lb /etc/expressgateway/config.toml
   Restart=on-failure
   ExecReload=/bin/kill -HUP $MAINPID
   CapabilityBoundingSet=CAP_BPF CAP_NET_ADMIN CAP_NET_BIND_SERVICE
   AmbientCapabilities=CAP_NET_BIND_SERVICE
   LimitNOFILE=1048576
   ```
   Note: `CAP_BPF` and `CAP_NET_ADMIN` are placeholders for when the
   eBPF attach path ships; `CAP_NET_BIND_SERVICE` is the only
   capability needed today to bind `:80` / `:443`.

2. **sysctl tuning.** Minimum set for a dual-plane LB host:
   ```
   net.core.somaxconn = 65535
   net.core.netdev_max_backlog = 65535
   net.ipv4.tcp_max_syn_backlog = 65535
   net.ipv4.tcp_tw_reuse = 1
   net.ipv4.ip_local_port_range = 1024 65535
   fs.file-max = 2097152
   ```

3. **Config.** Ship a minimal `/etc/expressgateway/config.toml`:
   ```
   [[listeners]]
   address = "0.0.0.0:8080"
   protocol = "tcp"
   [[listeners.backends]]
   address = "10.0.1.10:8080"
   weight = 1
   ```

4. **Metrics scraping.** Until the `/metrics` endpoint is wired,
   export counters via the JSON-formatted `tracing` output and
   aggregate at the log-shipper (vector, fluentd).

5. **TLS.** Front the binary with a TLS-terminating sidecar (Envoy,
   HAProxy) until the rustls integration lands. This is the
   recommended production topology until deferred item #1 ships.

6. **Observability.** Enable the JSON `tracing` formatter by
   setting `RUST_LOG=info,lb=debug` at startup. The
   `MetricsRegistry` is accessible in-process today; a future PR
   will wire it to a `/metrics` HTTP listener.

7. **Upgrades.** Use full-process rolling restarts (behind a
   connection-draining upstream) until fd-passing ships. The
   library-level zero-drop reload is covered by
   `tests/reload_zero_drop.rs`.

## Sign-off checklist

| Item | Command | Expected | Observed 2026-04-22 |
|------|---------|----------|---------------------|
| cargo fmt --check | `cargo fmt --check` | 0 | 0 (per halting gate) |
| cargo clippy | `cargo clippy --all-targets --all-features -- -D warnings` | 0 | 0 (per halting gate) |
| cargo test | `cargo test --all --all-features --no-fail-fast` | 0 failures | 296 passed / 0 failed / 0 ignored |
| Required-tests | manifest grep | 59/59 | 59/59 |
| Required-artifacts | `ls` each line | 141/141 | 141/141 once this PR merges |
| cargo deny | `cargo deny check` | 0 | 0 (per halting gate) |
| Panic grep | halting-gate step 3 | empty | empty |
| Manifest integrity | `sha256sum -c .halting-gate.sha256` | OK | OK |
| Halting gate | `bash scripts/halting-gate.sh` | exit 0 | will exit 0 after this PR |

## Sources

- `PROMPT.md` (the frozen specification).
- `scripts/halting-gate.sh` (the mechanical release gate).
- `manifest/required-artifacts.txt`, `manifest/required-tests.txt`,
  `.halting-gate.sha256` (closed, hash-locked scope).
- `deny.toml` (supply-chain policy).
- `docs/architecture.md` (design intent).
- `docs/gap-analysis.md` (honest status per PROMPT.md §28 row).
- `docs/research/pingora.md`, `docs/research/katran.md`, and the
  RFC studies in `docs/research/` (background for the design
  choices).
- `docs/decisions/ADR-0001` through `ADR-0010` (specific design
  decisions; io-uring crate, h2 codec strategy, quiche integration,
  eBPF framework, BPF map schema, frame pipeline, compression
  crates, control-plane protocol, graceful reload, panic-free
  enforcement).
- `crates/lb-*/src/**/*.rs` — authoritative source truth.
- `tests/*.rs` — authoritative behavioural truth.
