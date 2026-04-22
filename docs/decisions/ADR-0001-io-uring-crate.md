# ADR-0001: Async I/O runtime — tokio (epoll) over tokio-uring and glommio

- Status: Accepted (realized 2026-04-22)
- Date: 2026-04-22
- Deciders: ExpressGateway team
- Consulted: Pingora architecture notes, tokio documentation, tokio-uring README, glommio README, "Missing Manuals — io_uring worker pool"

## Context and problem statement

ExpressGateway is an L4/L7 load balancer written in Rust whose hot path is
overwhelmingly socket I/O: accept, read, write, splice between a downstream
connection and an upstream connection. The choice of async runtime therefore
has outsized consequences for throughput, tail latency, operability across
Linux kernel versions, and the size of the dependency graph we have to audit
under a panic-free regime.

Three realistic Rust async runtimes dominate the space: `tokio` backed by
mio/epoll, `tokio-uring` which layers io_uring completion-based semantics on
top of a tokio current-thread runtime, and `glommio`, a thread-per-core
runtime built directly on io_uring with shared-nothing semantics. Each makes
different bets about kernel capabilities, thread topology, and ecosystem
compatibility. We must pick one, and picking one closes off a lot of
downstream APIs (for instance, tokio-uring owned-buffer I/O traits are
incompatible with mio-flavoured `AsyncRead`/`AsyncWrite`).

The tension is between raw performance on a bleeding-edge kernel and being
deployable, auditable, and maintainable on the kernels that real operators
actually run (RHEL 9 ships 5.14; Debian 12 ships 6.1; Amazon Linux 2 still
ships 4.14 + backports). io_uring's security posture has also shifted: many
hardened distros disable it entirely via `kernel.io_uring_disabled` sysctl,
and Google disabled io_uring in production ChromeOS and Android in 2023 due
to a string of privilege-escalation CVEs.

## Decision drivers

- Kernel portability: must run on 4.19+ at minimum; must not silently
  degrade on hardened kernels where io_uring is disabled.
- Multi-threaded work-stealing runtime: load balancers are fundamentally
  multi-connection; thread-per-core sharding (glommio) penalises long-lived
  connections that drift.
- Ecosystem compatibility: `tokio-rustls`, `tokio-util::codec`, `hyper`,
  `tonic`, `tracing` — our entire dependency graph (see Cargo.lock) assumes
  tokio's `AsyncRead`/`AsyncWrite`.
- Panic-free auditability: fewer, more vetted dependencies to deny-lint.
- Security posture: avoid runtimes that force a kernel feature operators may
  have intentionally disabled.
- Maturity: tokio is the reference implementation; tokio-uring is marked
  experimental in its own README; glommio is primarily maintained by DataDog.
- Team familiarity.

## Considered options

1. `tokio` (work-stealing multi-thread, epoll/kqueue) — the default.
2. `tokio-uring` — per-thread current-thread runtime with io_uring completion
   I/O and owned-buffer APIs.
3. `glommio` — thread-per-core, io_uring-only, shared-nothing runtime.
4. Hybrid: tokio on the control plane, glommio on the data plane.

## Decision outcome

Adopt option 1: `tokio` with its default mio/epoll driver, built as a
multi-threaded runtime. The I/O backend abstraction in `lb-io` leaves room
to add an io_uring backend later without touching call sites.

## Rationale

- Cargo.lock resolves exactly one async runtime: `tokio` (no `tokio-uring`,
  no `glommio`). Changing now would ripple through every crate.
- `crates/lb/src/main.rs` boots a multi-threaded tokio runtime explicitly
  via `tokio::runtime::Builder::new_multi_thread().enable_all()` — the
  multi-thread variant is load-bearing because a single accepted connection
  may be proxied for minutes and we cannot afford per-core pinning for the
  L7 path.
- `lb-io` already encodes the abstraction: `IoBackend { IoUring, Epoll }`
  with a `detect_backend()` that today always returns `Epoll`
  (`crates/lb-io/src/lib.rs` lines 57–59). Upgrading the detection logic to
  probe kernel features is a local change; picking tokio-uring today would
  require rewriting every async boundary.
- Tokio is battle-tested in Cloudflare's Pingora (epoll-based until
  recently), AWS Lambda, and Discord — matching our target "production L4/L7
  proxy" class.
- io_uring regressions on hardened kernels would turn a perf win into a
  boot failure. The epoll path works everywhere; an io_uring fast path can
  be added behind the existing enum when we have a CI matrix that exercises
  it.
- glommio's thread-per-core model conflicts with connection-affinity hashing
  policies (Maglev, ring-hash) that we implement in `lb-balancer` — we want
  to pin a *flow* to a backend, not a CPU.
- `h2`-crate compatibility: the hyperium `h2` crate and `rustls` both target
  tokio traits. Choosing tokio-uring would force us to re-implement or
  adapt these, which is explicitly out of scope.

## Consequences

### Positive
- Dependency graph stays small and vetted.
- Works on any Linux kernel we care about, plus macOS/BSD for developer
  ergonomics.
- Compatible with the existing `#[deny(clippy::unwrap_used, …)]` lint gate
  — tokio's public API surfaces panics only through macros we don't use
  (e.g. `select!` has a `biased` mode and panic-free alternatives).

### Negative
- We leave some peak throughput on the table on kernels where io_uring
  would shine (roughly 15–25 % on short-lived connection workloads per
  Cloudflare's published benchmarks).
- We cannot use zero-copy `splice(2)`/`io_uring_prep_splice` directly — we
  fall back to user-space buffer copies in `lb-l7`.

### Neutral
- The `IoBackend` enum is a stable seam; flipping the default later is a
  feature flag, not a rewrite.

## Implementation notes

- `crates/lb-io/src/lib.rs` — `IoBackend` enum and `detect_backend()`.
  As of Pillar 1, `detect_backend()` performs a live NOP roundtrip against
  the kernel: it constructs an `io_uring::IoUring` with 8 entries, submits
  a single `opcode::Nop` SQE tagged with `0xDEADBEEF`, reaps the matching
  CQE, and tears the ring down. Any failure (kernel too old,
  `kernel.io_uring_disabled=1`, seccomp, permission denied, etc.) is logged
  at `tracing::debug` and the call falls back to `IoBackend::Epoll`. Full
  ACCEPT / RECV / SEND / SPLICE operations and tokio integration are
  scoped for Pillar 1b (follow-up); the seam in `lb-io` is now live-probed
  rather than simulated.
- `crates/lb-io/src/ring.rs` — Linux-only NOP probe (`nop_roundtrip()`),
  exposed behind `#[cfg(target_os = "linux")]`.
- `crates/lb-io/src/sockopts.rs` — `ListenerSockOpts`, `BackendSockOpts`,
  `UdpSockOpts` and matching `apply_*` helpers covering the options listed
  in PROMPT.md §7 (`SO_REUSEADDR`, `SO_REUSEPORT`, `SO_RCVBUF`, `SO_SNDBUF`,
  `TCP_NODELAY`, `TCP_QUICKACK`, `SO_KEEPALIVE`, `TCP_FASTOPEN`,
  `TCP_FASTOPEN_CONNECT`, `UDP_GRO`, listen backlog). The Linux-only knobs
  use raw `libc::setsockopt` with per-call `SAFETY:` comments; the
  portable subset uses `socket2`'s `SockRef` builder.
- `crates/lb/src/main.rs:53–59` — multi-thread runtime construction
  without `#[tokio::main]` (the macro expands to an internal `.unwrap()`,
  which would violate check 3 of `scripts/halting-gate.sh`).
- Root `Cargo.toml` workspace dep: `tokio = { version = "1", features = ["full"] }`,
  `io-uring = "0.7"`, `socket2 = { version = "0.5", features = ["all"] }`,
  `libc = "0.2"`.

## Follow-ups / open questions

- When do we add a real io_uring probe? Gated on a CI runner with kernel
  ≥ 6.1 and the feature-detection crate choice (`io-uring` vs
  `rustix::io_uring`) is deferred.
- Should we expose the backend choice in `lb-config` as an operator override
  for testing? Tracked in `crates/lb-config` TODO list.

## Sources

- <https://tokio.rs/>
- <https://github.com/tokio-rs/tokio-uring> (experimental status)
- <https://github.com/DataDog/glommio>
- Cloudflare blog, "Pingora open-source" 2024.
- `Cargo.lock` — resolved async runtime inventory.
- Internal: `crates/lb-io/src/lib.rs`, `crates/lb/src/main.rs`.
