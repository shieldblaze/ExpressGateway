# Round 1 — Code Inventory (`code` reviewer)

Repo: `/home/ubuntu/Code/ExpressGateway` (worktree
`agent-a65b54d8cde4a318b`). Edition 2024, MSRV 1.85, workspace resolver
2. Build pinned via `rust-toolchain.toml` channel `1.85` with `rustfmt`
and `clippy`. 18 workspace crates plus one out-of-workspace `fuzz/`
crate.

This document is **inventory only** — no findings. Findings open in
Round 2 with stable IDs `CODE-2-NN`.

---

## 1. Module map

### 1.1 One-line purpose per crate

| Crate | LOC (src) | Purpose |
|---|---:|---|
| `lb-core` | 408 | Type vocabulary: `Backend`, `BackendHealth`, `BackendState` (atomic counters), `Cluster`, `LbPolicy`, `CoreError`. Independent of `lb-balancer`. |
| `lb-config` | 1 536 | TOML config schema + `parse_config` / `validate_config`. Listener/backend/TLS/H2-security/timeouts/Alt-Svc/WebSocket structs. |
| `lb-controlplane` | 469 | `ConfigBackend` trait, file + in-memory impls, `ConfigManager` with rollback (SIGHUP-style). Uses `std::sync::Mutex`. |
| `lb-cp-client` | 119 | Stub client → control plane. Currently a thin wrapper around an endpoint string; no network code yet. |
| `lb-l4-xdp` | 1 475 | Userspace XDP loader (`aya`), CI-safe simulator (`sim.rs`), and the eBPF program under `ebpf/` (`#![no_std] #![no_main]`, separate Cargo.lock, 51 unsafe blocks in `ebpf/src/main.rs`). |
| `lb-io` | 3 435 | I/O abstraction: backend probe (`detect_backend` → io_uring or epoll), `Runtime` facade, DNS resolver with cache, TCP / H2 / QUIC connection pools, socket-options helpers, `io_uring` accept/recv/send/splice wrappers (Linux). |
| `lb-h1` | 768 | HTTP/1.1 frame codec: request/status line parser, header parser with byte-limit, chunked encoder/decoder. Pure-data crate (no I/O). |
| `lb-h2` | 2 057 | HTTP/2 frame codec, HPACK encoder/decoder, and security detectors: Rapid-Reset, CONTINUATION-flood, HPACK-bomb, PING/SETTINGS flood, zero-window stall. |
| `lb-h3` | 1 048 | HTTP/3 frame codec, varint, QPACK encoder/decoder, QPACK-bomb detector. |
| `lb-quic` | 2 945 | QUIC datagram + stream roundtrip via `quiche` 0.28; `QuicListener` (real UDP), per-connection `conn_actor`, `router` dispatching by DCID via `DashMap`, `h3_bridge` for backend forwarding. |
| `lb-grpc` | 604 | gRPC framing (`frame.rs`), status codes, deadline parsing, server/client streaming codec. No `tonic`/`prost` dependency. |
| `lb-l7` | 5 086 | L7 proxy core. `H1Proxy`, `H2Proxy`, `WsProxy`, `GrpcProxy` plus the nine direction-pair bridges (`h{1,2,3}_to_h{1,2,3}.rs`). Wraps hyper 1.x. |
| `lb-balancer` | 1 356 | Stand-alone balancer algorithms: round-robin, weighted RR, weighted random, random, P2C, least-connections, least-request, EWMA, Maglev, ring-hash, session-affinity. |
| `lb-health` | 175 | Active health checks. Minimal today. |
| `lb-security` | 1 687 | `TicketRotator` for rustls session tickets, retry-token signer (BLAKE3-keyed), 0-RTT replay guard, slow-post / slowloris detectors, request-smuggling detector. |
| `lb-compression` | 658 | gzip / deflate / brotli / zstd transcode with bomb-cap + BREACH-posture controls. |
| `lb-observability` | 749 | `MetricsRegistry` (prometheus-rs handle cache), text-exposition renderer, admin HTTP listener (`GET /metrics`, `GET /healthz`) on hyper 1.x. |
| `lb` (`expressgateway` bin) | 1 515 | Binary entry point — builds Tokio multi-thread runtime, parses config, wires listeners, optionally attaches XDP. |

### 1.2 Inter-crate dependency graph (path-deps only)

```
lb (bin) ── lb-io, lb-config, lb-controlplane, lb-balancer,
            lb-observability, lb-health, lb-security,
            lb-quic, lb-l7, lb-l4-xdp
lb-l7   ── lb-io, lb-h2, lb-quic, lb-grpc
lb-quic ── lb-security, lb-io, lb-h3
```

All other crates are leaves; they pull external deps only. Notable
implicit fan-in:

- `lb-io` is depended on by `lb`, `lb-l7`, `lb-quic` — it is the
  central runtime/I/O service.
- `lb-security` is reachable only through `lb` and `lb-quic`. The H1/H2
  proxies in `lb-l7` do **not** pull `lb-security`; smuggling /
  slow-loris / slow-POST detectors are not wired into the L7 hot path
  (see Open Questions §6 — flag for sec).
- `lb-h1` is **not** referenced by any other workspace crate today.
  Only consumers are the fuzz target (`fuzz/fuzz_targets/h1_parser.rs`)
  and tests. The runtime HTTP/1.1 path uses hyper 1.x directly inside
  `lb-l7::h1_proxy`. (See §6 — possibly dead code or future use.)
- `lb-balancer` is consumed by `lb` (the binary picks round-robin) but
  **not** by `lb-l7::h1_proxy` / `h2_proxy` — those use their own
  `RoundRobinUpstreams` defined in `lb-l7::upstream`. Algorithm code
  duplication between crates needs review.
- `lb-controlplane` and `lb-cp-client` are present in the workspace
  but the binary doesn't appear to consume `lb-controlplane` for live
  reload yet — confirm in Round 2.

### 1.3 Request lifecycle (entry points)

**TCP / TLS / H1 / H1s listener** — `crates/lb/src/main.rs`:

1. `main` → `tokio::runtime::Builder::new_multi_thread().enable_all().build()` at `crates/lb/src/main.rs:921`.
2. `async_main` parses config, builds `lb_io::Runtime`, `TcpPool`, `DnsResolver`, `MetricsRegistry`, then for each listener `spawn_listener` returns `JoinHandle` (`main.rs:701`).
3. `run_listener` (`main.rs:1077`) loops `TcpListener::accept`, picks a backend via the per-listener `parking_lot::Mutex<RoundRobin>`, then `tokio::spawn`s the connection handler (`main.rs:1126`).
4. Handler matches `ListenerMode` (`PlainTcp` / `Tls` / `H1` / `H1s` / `H2` / `H2s` / `Grpc` / `Ws` / `Wss`). `Tls` and `H1s` modes call `TlsAcceptor::accept`, then either `proxy_connection` (raw splice loop) or `H1Proxy::serve_connection` / `H2Proxy::serve_connection`.
5. `proxy_connection` (`main.rs:1219`) acquires a pooled upstream via `tokio::task::spawn_blocking(move || pool.acquire(addr))` (`main.rs:1238`) — the pool's `acquire` is blocking by design — and bridges with `tokio::io::copy_bidirectional`.

**L7 H1 proxy** — `crates/lb-l7/src/h1_proxy.rs`:

1. `H1Proxy::serve_connection` (`h1_proxy.rs:285`) wraps the inbound stream in `hyper::server::conn::http1::Builder` and serves a hyper `Service`.
2. `proxy_request` (`h1_proxy.rs:413`) strips hop-by-hop headers (RFC 9110 §7.6.1), adds X-Forwarded-{For,Proto,Host} + Via, picks a backend via the configured `BackendInfoPicker`, acquires a pool slot via `spawn_blocking` (`h1_proxy.rs:419`), then spawns the upstream connection driver (`h1_proxy.rs:432`).
3. WebSocket upgrades branch through `is_h1_upgrade_request` → `run_h1_ws_upgrade_task` (`h1_proxy.rs:581`).

**L7 H2 proxy** — `crates/lb-l7/src/h2_proxy.rs`:

1. `H2Proxy::serve_connection` (`h2_proxy.rs:221`) drives hyper 1.x H/2 server.
2. `proxy_request` (`h2_proxy.rs:466`) — h2→h1, h2→h2 (`Http2Pool`), or h2→h3 (`QuicUpstreamPool`) per direction.

**QUIC listener** — `crates/lb-quic/src/listener.rs`:

1. `QuicListener::bind` opens UDP socket, builds `RetryTokenSigner`, `ZeroRttReplayGuard`, then `router::spawn`.
2. `router_main` (`router.rs:121`) `select!`s `cancel.cancelled()` vs `socket.recv_from`, decodes header, looks up DCID in `DashMap<Vec<u8>, mpsc::Sender<InboundPacket>>`, dispatches initial packets via `dispatch_initial` (`router.rs:374` spawns the per-connection actor).
3. `conn_actor::run_actor` (`conn_actor.rs:138` select! loop) drives quiche per connection, spawning a per-request bridge task into `lb-quic::h3_bridge` (`conn_actor.rs:296`, `:307`, `:320`).

**XDP fast path** — `crates/lb-l4-xdp/`:

1. `lb/src/xdp.rs::try_attach_xdp` loads the BPF ELF (`lb_xdp.bin` committed in src/) via `aya`. Linux-only.
2. eBPF program (`lb-l4-xdp/ebpf/src/main.rs`) parses Ethernet/VLAN/IPv4/IPv6/TCP/UDP, consults `CONNTRACK`, `L7_PORTS`, `ACL_DENY_TRIE`, performs MAC/IP/port rewrite with RFC 1624 incremental checksum, then `XDP_TX`. Bounds failure → `XDP_PASS`.

---

## 2. Concurrency model

### 2.1 Tokio runtime topology

- One multi-thread runtime built in `main` (`crates/lb/src/main.rs:921`). No second runtime. `#[tokio::main]` is intentionally avoided because the macro adds a hidden `.unwrap()` (comment at `main.rs:917`).
- Worker count: not configured — defaults to `available_parallelism()`.
- `enable_all()` is set, so both `time` and `io` drivers are present.
- `lb-io::Runtime` (`crates/lb-io/src/lib.rs:239` test path) is a **non-Tokio** facade; it just records the chosen I/O backend and watermark constants.

### 2.2 `tokio::spawn` sites — full inventory

(33 `tokio::spawn` + `tokio::task::spawn` sites listed below; one
extra `spawn_blocking` mention is in a doc comment, so the raw
grep count of `tokio::spawn|task::spawn_blocking` is 36.)

| File:line | Owner | Shutdown path | Notes |
|---|---|---|---|
| `crates/lb/src/main.rs:233` | `spawn_rotator_ticker` | Exits when `Arc::strong_count(&rotator) <= 1` (cooperative). | TLS-ticket rotator; 60-s tick. |
| `crates/lb/src/main.rs:701` | `spawn_listener` | None — joined only on process exit. | One per listener — `JoinHandle` stored in main. |
| `crates/lb/src/main.rs:892` | metrics sampler | Loops forever; no cancel. | Samples pool / DNS gauges every 1 s. |
| `crates/lb/src/main.rs:1126` | per-connection in `run_listener` | None — relies on stream close / RST. | Hot-path connection handler. |
| `crates/lb/src/main.rs:1238` | `spawn_blocking` (pool acquire) | Awaited inline. | Blocking `pool.acquire`. |
| `crates/lb-l7/src/grpc_proxy.rs:214` | `spawn_blocking` (pool acquire) | Awaited inline. | |
| `crates/lb-l7/src/grpc_proxy.rs:250` | upstream conn driver | None — relies on driver finishing. | |
| `crates/lb-l7/src/h2_proxy.rs:400` | per-request fan-out | None — relies on completion. | |
| `crates/lb-l7/src/h2_proxy.rs:410` | `spawn_blocking` (pool acquire) | Awaited inline. | |
| `crates/lb-l7/src/h2_proxy.rs:472` | `spawn_blocking` (pool acquire) | Awaited inline. | |
| `crates/lb-l7/src/h2_proxy.rs:487` | upstream conn driver | None. | |
| `crates/lb-l7/src/h1_proxy.rs:419` | `spawn_blocking` (pool acquire) | Awaited inline. | |
| `crates/lb-l7/src/h1_proxy.rs:432` | upstream conn driver | None. | |
| `crates/lb-l7/src/h1_proxy.rs:581` | WS-upgrade task | None — ws_proxy `select!` exits on idle timeout. | |
| `crates/lb-l7/src/h1_proxy.rs:659` | `spawn_blocking` (pool acquire) | Awaited inline. | |
| `crates/lb-l7/src/ws_proxy.rs:618` | inner WS forwarder | Idle-timeout via `select!`. | |
| `crates/lb-observability/src/admin_http.rs:102` | admin accept loop | `CancellationToken`. | Returns on `shutdown.cancelled()`. |
| `crates/lb-observability/src/admin_http.rs:120` | per-admin-connection | None. | |
| `crates/lb-io/src/dns.rs:283` | `spawn_blocking` (resolver) | Awaited inline. | |
| `crates/lb-io/src/dns.rs:337` | empty `async {}` | Trivially completes. | Stub returned when no work. |
| `crates/lb-io/src/dns.rs:340` | DNS lookup task | None. | Per-request. |
| `crates/lb-io/src/dns.rs:499` | DNS batch | Joined. | Test code path. |
| `crates/lb-io/src/http2_pool.rs:261` | `spawn_blocking` (pool acquire) | Awaited inline. | |
| `crates/lb-io/src/http2_pool.rs:284` | H2 connection driver | None — exits on conn close. | |
| `crates/lb-quic/src/lib.rs:706` | server datagram task | Joined by caller. | Roundtrip helpers (test/sim). |
| `crates/lb-quic/src/lib.rs:778` | server stream task | Joined by caller. | |
| `crates/lb-quic/src/router.rs:113` | router task | `JoinHandle` returned via `RouterHandle::join`. | Cancellable via `params.cancel`. |
| `crates/lb-quic/src/router.rs:374` | per-connection actor | Self-cleans `DashMap` entries on exit. | Spawned by `dispatch_initial`. |
| `crates/lb-quic/src/listener.rs:249` | listener wrapper task | `CancellationToken` + `JoinHandle`. | Awaited by `QuicListener::shutdown()`. |
| `crates/lb-quic/src/conn_actor.rs:296` | h3→h2 per-request | None — completes on response. | |
| `crates/lb-quic/src/conn_actor.rs:307` | h3→h3 per-request | None. | |
| `crates/lb-quic/src/conn_actor.rs:320` | h3→h1 per-request | None. | |
| `crates/lb-quic/src/h3_bridge.rs:275` | `spawn_blocking` (pool acquire) | Awaited inline. | |

**Cancellation discipline summary:** only the admin HTTP listener, the
QUIC listener, and the QUIC router check a `CancellationToken`. The
two HTTP listener accept-loops in `crates/lb/src/main.rs:1098` /
`spawn_rotator_ticker` / metrics sampler / every per-connection task
exit on stream close, with no graceful drain. This is open for Round 2
review (likely `CODE-2-` finding: missing process-wide graceful
shutdown). See cross-ref to `rel` (reliability) in §6.

### 2.3 Hand-rolled sync primitives

- `parking_lot::Mutex` — used in `crates/lb/src/main.rs` (RoundRobin pick under hot-path mutex per listener), `lb-quic/src/{router,listener,conn_actor}.rs`, `lb-security/src/ticket.rs` (rotator), `lb-l7/src/h1_proxy.rs`, `lb-io/src/{dns,pool,http2_pool,quic_pool}.rs`, `lb-l7/src/upstream.rs`. ~10 files.
- `std::sync::Mutex` — `lb-controlplane/src/lib.rs:21` (config manager). Open question for sec/rel: poison handling not yet inspected.
- `dashmap::DashMap` — `lb-quic/src/router.rs` (CID → actor channel), `lb-observability/src/lib.rs` (registry handle cache). Both as `Arc<DashMap<…>>`.
- `arc-swap` — declared in `Cargo.toml [workspace.dependencies]` but **no `ArcSwap` / `arc_swap::` usage in the workspace** (zero hits in `crates/`). Likely a vestigial dependency. Flag for Round 2 (`cargo machete` candidate); see §3.
- `tokio::sync::mpsc` / `oneshot` / `broadcast` / `watch` — only `mpsc` is used, in `lb-quic/src/router.rs` and `lb-quic/src/conn_actor.rs` (`ACTOR_CHANNEL_DEPTH`-bounded per-connection channel). No `oneshot`/`broadcast`/`watch` anywhere — confirm in Round 2 whether reload-fanout is missing or implemented externally.
- `tokio_util::sync::CancellationToken` — admin HTTP, QUIC listener, QUIC router. Five files total.
- No `crossbeam`, no `seqlock`, no spin loops, no `AtomicPtr` anywhere in the workspace.

### 2.4 `unsafe` inventory

- **`crates/lb-l4-xdp/src/loader.rs:53,76,97,120`** — 4 `unsafe impl aya::Pod for {FlowKey,BackendEntry,FlowKeyV6,BackendEntryV6}`. Each is a `#[repr(C)] Copy` plain-data struct mirroring an eBPF map layout. Justification comments are present; verify in Round 2 there are no implicit padding bytes (FlowKey explicitly carries `pad: [u8; 3]`).
- **`crates/lb-io/src/ring.rs`** — 17 `unsafe` blocks wrapping the `io_uring` submission queue (`push_sqe`), opcode dispatch, and `sockaddr_storage` → `SocketAddr` conversion. The accepted-fd ownership story uses raw `libc::close` on error (line 345). This is the highest-risk Rust file in the workspace from a memory-safety POV.
- **`crates/lb-l4-xdp/ebpf/src/main.rs`** — 51 `unsafe` blocks. Standard XDP pattern: every packet pointer dereference must be bounds-checked first, hence `ptr_at::<T>(ctx, offset)` returns `Option<*const T>` and reads are `core::ptr::read_unaligned(addr_of!(...))`. Out of workspace (separate Cargo.lock) — clippy/MSRV checks above do **not** cover this crate. Pure-`ebpf` work owned by `ebpf` teammate; ping them in cross-talk.
- **No `unsafe impl Send for`/`Sync for`** anywhere in the workspace. Good — the only `unsafe impl`s are for `aya::Pod`.

### 2.5 Every `Ordering::Relaxed` site

50 atomic-ordering tokens in the workspace; **all 50 use `Ordering::Relaxed`**. There is **no use of `Acquire`, `Release`, `SeqCst`, or `AcqRel` anywhere in the source.** Full list:

| File | Lines |
|---|---|
| `crates/lb-core/src/backend.rs` | 87, 92, 98, 104, 105, 116, 121, 126, 132, 133, 144, 149, 162, 163, 164 (3× saturating-sub CAS loops use Relaxed/Relaxed for both success and failure orderings) |
| `crates/lb/src/main.rs` | 1127, 1212 (per-connection active-count gauge) |
| `crates/lb-io/src/pool.rs` | 113, 141, 185, 372, 389, 395, 455 |
| `crates/lb-io/src/quic_pool.rs` | 184, 217, 230, 236, 271, 283, 403, 405, 499, 516, 518, 566, 587 |
| `crates/lb-io/src/dns.rs` | 396, 416, 434, 438, 451, 467, 475, 489, 508, 530, 532 |

**Round-2 audit-target list:**

1. `lb-core::BackendState::dec_connections` / `dec_requests`
   (`backend.rs:98–110`, `:125–137`) — saturating-decrement
   `compare_exchange_weak` with `(Relaxed, Relaxed)`. Correct for a
   pure counter, **but** any consumer reading `active_connections()`
   to *decide* something (P2C / least-conn balancer) needs Acquire
   semantics to observe writes ordered with respect to the increment
   site. Confirm whether the balancer crates use this counter or a
   private one. (Currently `lb-balancer` carries its own `u64`
   fields in its own `Backend` struct; the relationship to
   `lb-core::Backend` is loose — duplication noted in §1.2.)
2. `lb-io::pool` `total.load(Relaxed) >= total_max` capacity gate at
   `pool.rs:372` — this is a classic check-then-act under Relaxed.
   Possible TOCTOU permitting overshoot; mitigated by `fetch_add`
   after the gate. Need to confirm the increment isn't lost.
3. `lb-io::quic_pool` evict-then-replace path at `quic_pool.rs:516`
   (`fetch_sub(evicted_total)` then `fetch_add(1)`): without a happens-before
   relationship to the queue mutation, observers can see total
   transiently decrement and see the queue not yet shrunk.

These do **not** need to block any release on their own — most are
*statistics* — but the policy of *every* atomic being Relaxed is a
red flag for Round 2.

### 2.6 Blocking calls inside async / raw threads

- `crates/lb-observability/src/lib.rs:484` — `std::thread::spawn` inside a `#[test]` (concurrency test for the registry). Test-only; fine.
- `crates/lb-io/src/ring.rs:333,356,399` — `std::thread::spawn` inside `#[cfg(test)] mod tests` echo listener. Test-only.
- `crates/lb-io/src/dns.rs:490` — `std::thread::sleep(30ms)` inside a test that races two `tokio::task::spawn_blocking` resolver calls. Test-only.
- All `pool.acquire()` calls in production code paths are wrapped with `tokio::task::spawn_blocking` — good. The exception worth flagging is `crates/lb-quic/src/listener.rs:213` where an in-line `lb_io::Runtime::new()` is built when no pool is supplied (smoke path only).

---

## 3. Build hygiene

### 3.1 Tooling availability

- `rustc 1.85.1` and `cargo 1.85.1` available at
  `/home/ubuntu/.rustup/toolchains/1.85-x86_64-unknown-linux-gnu/`.
  rustup self-update is **broken in this sandbox** (component
  rollback errors on rename); workaround is `RUSTUP_TOOLCHAIN=1.85-...`
  in env to bypass the shim's component sync. **Action for `rel`**:
  ensure CI / docker images bake the toolchain to skip rustup.
- `cargo fmt`, `cargo clippy`, `rustfmt`, `clippy-driver` all present in the toolchain bin dir.
- `cargo-machete` is **not installed**, and `crates.io` install kicked off but not relied on here. (Will retry in Round 2.) Until then, `cargo-machete` results are absent from this inventory.
- The build needed `gcc` + `cmake` to compile `boring-sys` (transitively via `tokio-quiche`); both were missing from this sandbox at start and were installed (`gcc 15.2.0`, `cmake 4.2.3`). Document this for `rel`.

### 3.2 `cargo fmt --check`

```
$ RUSTUP_TOOLCHAIN=1.85-... cargo fmt --check
(no output)
EXIT:0
```

Clean.

### 3.3 `cargo clippy --all-targets --all-features -- -D warnings`

```
$ RUSTUP_TOOLCHAIN=1.85-... cargo clippy --all-targets --all-features -- -D warnings
… (107 lines of "Checking …" / "Compiling …" progress) …
EXIT:0
```

**0 errors, 0 warnings, exit code 0.** Full log captured in
`audit/code/round-1-clippy.txt`. The workspace is clippy-clean at
`-D warnings` with `--all-features` on Rust 1.85.1 — including all
test targets. This is a meaningful baseline: any Round-2 finding that
would touch this needs to either add a new lint with justification
or stay green.

### 3.4 `cargo machete`

`cargo-machete 0.9.2` installed via stable toolchain. Full output in
`audit/code/round-1-machete.txt`. Suspected-unused deps reported per
crate:

| Crate | Suspected-unused deps |
|---|---|
| `lb` (bin) | `hyper`, `hyper-util`, `lb-controlplane`, `lb-health` |
| `lb-balancer` | `parking_lot` |
| `lb-l4-xdp` | `parking_lot`, `rand` |
| `lb-controlplane` | `serde`, `serde_json` |
| `lb-core` | `bytes` |
| `lb-io` | `bytes` |
| `lb-grpc` | `http` |
| `lb-h2` | `http` |
| `lb-l7` | `http` |
| `lb-health` | `tokio` |
| `lb-compression` | `bytes` |
| `lb-security` | `bytes`, `http` |
| `lb-xdp-ebpf` (out-of-workspace) | `aya-log-ebpf` |

Notes for Round 2 (`code` finding candidates):

- `lb`'s unused `lb-controlplane` + `lb-health` confirms the binary is
  not yet wiring the control-plane reload path or health endpoints —
  hand to `rel` (re-confirms Q-CODE-1-04).
- `http` unused in `lb-l7` and `lb-h2` is a false-positive risk
  because those crates routinely re-export hyper types — verify with
  `--with-metadata`. Same for `serde`/`serde_json` in
  `lb-controlplane` (likely used through derive macros).
- `arc-swap` is declared in `[workspace.dependencies]` but not
  referenced in `crates/` (manual check, not flagged by machete which
  only audits per-crate manifests). Round 2 should drop it.

### 3.5 Build / lint results — summary

| Tool | Command | Result |
|---|---|---|
| rustfmt | `cargo fmt --check` | clean, exit 0 |
| clippy | `cargo clippy --all-targets --all-features -- -D warnings` | clean, exit 0 (0 err, 0 warn) |
| cargo check | implicit via clippy (same compile graph) | clean, exit 0 |
| cargo machete | (install pending — sandbox blocker, see §3.1) | deferred |

Build environment: `rustc 1.85.1`, `cargo 1.85.1`, `gcc 15.2.0`,
`cmake 4.2.3`, `clang 21.1`. Sandbox initially lacked `gcc` / `cmake`
/ `libclang-dev` / `libc6-dev`; these were installed (apt) to allow
`boring-sys` (BoringSSL via `tokio-quiche`) and `aya-obj` to build.
Flag this as an issue for `rel` — CI / docker images should bake the
full C toolchain.

---

## 4. Test inventory

### 4.1 Integration tests (root `tests/`)

68 integration test files, grouped:

- **Balancing**: 11 (`balancer_*.rs`) — one per algorithm.
- **L4 XDP**: 3 (`l4_xdp_conntrack.rs`, `l4_xdp_hotswap.rs`, `l4_xdp_maglev.rs`).
- **L7 protocols**: 9 H1↔H{1,2,3} / H2↔H{1,2,3} / H3↔H{1,2,3} bridging tests + 3 `conformance_h{1,2,3}.rs` + `proto_translation_e2e.rs` + `binary_proto_routing.rs`.
- **L7 e2e**: `h1_proxy_e2e.rs`, `h2_proxy_e2e.rs`, `ws_proxy_e2e.rs`, `ws_autobahn.rs`, `quic_listener_e2e.rs`, `quic_native.rs`, `tls_listener.rs`, `metrics_endpoint.rs`, `reload_zero_drop.rs`.
- **gRPC**: 8 (`grpc_*.rs`) — unary, server-streaming, client-streaming, bidi, deadline, status translation, external client, proxy-e2e.
- **Compression**: 7 (`compression_*.rs`) including `compression_bomb_cap.rs` and `compression_breach_posture.rs`.
- **Security**: 11 (`security_*.rs`) — Rapid-Reset, CONTINUATION flood, HPACK bomb, QPACK bomb, slow-post, slowloris, smuggling CL-TE/TE-CL, H2 downgrade, zero-RTT replay, plus `h2spec.rs` (live h2 conformance) and `h2_security_live.rs`.
- **Control-plane**: 3 (`controlplane_ha.rs`, `controlplane_rollback.rs`, `controlplane_standalone.rs`).
- **H3 upstream**: `h3_upstream_verify.rs`.

### 4.2 Unit-test density per crate

| Crate | `#[test]` + `#[tokio::test]` |
|---|---:|
| `lb-l7` | 55 |
| `lb-io` | 41 |
| `lb-security` | 36 |
| `lb-config` | 31 |
| `lb-h2` | 30 |
| `lb-l4-xdp` | 24 |
| `lb-balancer` | 22 |
| `lb-grpc` | 21 |
| `lb-h3` | 19 |
| `lb-controlplane` | 15 |
| `lb-h1` | 14 |
| `lb-observability` | 13 |
| `lb-compression` | 10 |
| `lb-core` | 10 |
| `lb-quic` | 7 |
| `lb-health` | 6 |
| `lb` (bin) | 5 |
| `lb-cp-client` | 4 |

Lowest densities (relative to LOC): `lb-quic` (7 unit tests over 2 945
LOC — most coverage lives in integration tests against the live UDP
listener), `lb-health` (175 LOC, 6 tests), `lb-cp-client` (stub).

### 4.3 Proptest / loom / miri

- **Zero** `proptest` references in `crates/`.
- **Zero** `loom` references in `crates/`.
- **Zero** `miri`-gated tests. `Cargo.toml` files contain no
  `cfg(miri)` cfg-guards.
- `crates/lb-io/src/pool.rs` contains the only hit — but it is a
  prose comment, not a `proptest!` block.

Gap for Round 2 (`code` finding candidate): every `lb-h{1,2,3}` parser
and HPACK/QPACK decoder is a prime target for property testing, and
every lock-free pattern (`lb-core::backend.rs` saturating-CAS) is a
prime target for `loom`. Currently neither is exercised.

### 4.4 Fuzz targets

`fuzz/` is out-of-workspace; nightly-libfuzzer toolchain pinned via
`fuzz/rust-toolchain.toml`. Targets:

| Target | Corpus inputs |
|---|---:|
| `h1_parser` | 5 |
| `h2_frame` | 7 |
| `h3_frame` | 7 |
| `quic_initial` | 5 |
| `tls_client_hello` | 5 |

`findings/` contains one `*.smoke.txt` per target — single sanity
input proven to run, not real coverage. No persistent crashes
checked in.

Missing fuzz targets that I would expect for a production L7 LB:
HPACK decoder (bounded-by-table), QPACK decoder, chunked-transfer
decoder, request-smuggling detector, TLS-ticket decoding, gRPC frame
parser, WebSocket frame parser, varint decoder. Hand these to
`proto` for evaluation; they will likely re-derive the same list.

### 4.5 Coverage gaps (quick read)

- `lb-quic`: integration-heavy, unit-light; very little of the QUIC
  state machine has direct unit coverage. Needs cross-talk with
  `proto`.
- `lb-l4-xdp` userspace loader: 24 unit tests, but they hit only the
  simulator (`sim.rs`); the real `loader.rs` cannot be exercised
  without a privileged kernel. `ebpf/` has its own minimal tests.
  `ebpf` teammate owns.
- `lb-controlplane`: 15 tests, but the file-backed store has no
  fault-injection or partial-write-failure paths. Cross-talk to `rel`.
- `lb-balancer` vs `lb-core::BackendState`: duplicate `u64` counter
  fields without a coupling test. Round-2 `CODE` finding candidate.

---

## 5. MSRV

`Cargo.toml`:

```
[workspace.package]
rust-version = "1.85"
```

`rust-toolchain.toml` pins `1.85` channel with `rustfmt` and `clippy`.

`Cargo.lock` pin notes (workspace `Cargo.toml` lines 21–25): `foundations
4.5.0` and `idna_adapter 1.1.0` are deliberately frozen to prevent a
transitive rustc 1.86 / 1.88 bump.

`cargo check --workspace` requires the same `cc`/`cmake` chain as
clippy; the clippy run above also performs `cargo check` semantics, so
once §3.5 reports green, MSRV 1.85 stands confirmed on this host.

---

## 6. Open questions for other teammates

Q-CODE-1-01 (→ `sec`): `lb-security` exposes `SlowPostDetector`,
`SlowlorisDetector`, `SmuggleDetector`, `RetryTokenSigner`,
`ZeroRttReplayGuard`, but `lb-l7` does **not** depend on `lb-security`
in `Cargo.toml`. Are these detectors wired in elsewhere (e.g. inside
hyper-side hooks I haven't found yet) or are they unwired? If unwired
on the request path, this is a serious gap.

Q-CODE-1-02 (→ `sec`): Every atomic in the workspace uses
`Ordering::Relaxed`. Confirm whether any of the counter-based decisions
on the security side (per-IP rate limits, rapid-reset thresholds in
`lb-h2::security`) require Acquire/Release. If any *enforcement*
threshold depends on a counter, Relaxed is wrong.

Q-CODE-1-03 (→ `ebpf`): `crates/lb-l4-xdp/ebpf/` has its own Cargo.lock
and rust-toolchain. Workspace clippy does **not** lint it. Are you
running `cargo clippy` inside `ebpf/` independently? I'll skip it in
my Round-2 sweep otherwise. Also: 51 `unsafe` blocks in
`ebpf/src/main.rs` need your sign-off.

Q-CODE-1-04 (→ `rel`): Only the admin HTTP listener and QUIC listener
honour a `CancellationToken`. The TCP/H1/H1s listener `tokio::spawn`
at `crates/lb/src/main.rs:701` is never cancelled — only the process
exiting drops the `JoinHandle`. Is the intent that SIGTERM aborts
immediately? If graceful drain is required, this is a `rel` finding;
I'll defer to your call before writing my own.

Q-CODE-1-05 (→ `rel`): `crates/lb/src/main.rs:892` spawns a 1-second
metrics sampler with no cancellation. On runtime drop it should
finish, but it is worth confirming with `tokio::time::interval` +
`select! { _ = cancel.cancelled() => break, … }`.

Q-CODE-1-06 (→ `proto`): `lb-h1` is not depended on by any
workspace member except the fuzz target (`fuzz/fuzz_targets/h1_parser.rs`).
Are you using it as a spec parser (cross-verification with hyper's
parser) or is this dead code? If the latter, drop. If the former,
make the dependency edge explicit.

Q-CODE-1-07 (→ `proto`): `lb-balancer` and `lb-core` each carry a
`Backend` struct with overlapping `u64` counter fields. Are the
balancer algorithms supposed to consume `lb-core::BackendState`'s
atomics, or maintain their own? The duplication today is a
correctness hazard (drift between the two).

Q-CODE-1-08 (→ `sec`, `proto`): `lb-h{1,2,3}` parsers all have
`#![deny(clippy::unwrap_used, …, clippy::indexing_slicing)]` —
excellent — but the fuzz corpora are tiny (5-7 inputs each). I'd like
to see HPACK/QPACK/chunked/varint added as targets. Are you planning
to expand the fuzz surface in Round 2?

Q-CODE-1-09 (→ `sec`): `crates/lb-quic/src/router.rs:374` per-CID
actor leaves `DashMap` entries behind if the actor panics before
`remove(&router_key)`. The comment says this is "acceptable" since
they get reaped on next `try_send` failure. Confirm this is OK from
your end — under sustained adversarial Initial flooding this is a
slow memory creep until reap.

---

## 7. `cargo-machete` results

See §3.4 — the report is in. Top items to confirm in Round 2:

- `arc-swap` declared in `[workspace.dependencies]` but unused (manual
  finding — machete doesn't audit workspace-level deps).
- `lb`'s suspected-unused `lb-controlplane` + `lb-health` — the bin
  hasn't wired the reload + active-health paths.
- `http` flagged in `lb-h2` / `lb-l7` / `lb-grpc` is likely a
  re-export false positive; verify with `--with-metadata`.
