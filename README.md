# ExpressGateway

A globally distributed load balancer written in Rust, featuring:

- **L7 data plane**: HTTP/1.1, HTTP/2, HTTP/3 with frame-by-frame processing.
- **L4 data plane**: XDP/eBPF-based TCP and UDP load balancing (loader
  in `lb-l4-xdp`; userspace loader integrated, BPF ELF build behind
  the toolchain caveat in `DEPLOYMENT.md`).
- **QUIC-native proxying** via quiche.
- **Native gRPC** proxy with full streaming support.
- **WebSocket** with H1 `Upgrade` and H2 extended-CONNECT (RFC 8441).
- **11 load-balancing algorithms**: round-robin, weighted round-robin,
  random, weighted random, P2C, Maglev, ring hash, EWMA, least
  connections, least requests, session affinity.
- **Active and passive health checking**.
- **DoS-mitigation catalog**: Rapid-Reset, CONTINUATION flood, HPACK /
  QPACK bomb, SETTINGS flood, PING flood, zero-window stall, slow-loris,
  slow-POST, request smuggling (CL.TE, TE.CL, H2 downgrade), zero-RTT
  replay.
- **File-backed control plane** with a trait seam for future
  distributed backends.
- **Standalone and HA modes**.
- **Zero-downtime reload** via `SO_REUSEPORT` and FD passing.
- **Panic-free libraries**: every `crates/*/src/lib.rs` carries
  `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, ...)]`;
  CI's `panic-freedom` job enforces it. The binary (`crates/lb`) carries
  the same deny lints and installs a `std::panic::set_hook` that bumps
  `panic_total` and emits a structured `tracing::error!` before the
  process continues. The `expect`/`unwrap` calls remaining in source
  live inside `#[cfg(test)] mod tests { ... }` blocks.
- **Graceful drain on SIGTERM**: lameduck `/readyz` flip → settle →
  cancellation-token cancel → bounded wait → `shutdown_aborted_connections_total`
  bump on overflow. See `RUNBOOK.md`.

## Building

```bash
cargo build --release -p lb --bin expressgateway
```

The produced artifact is `target/release/expressgateway`. See
`DEPLOYMENT.md` for system-level prerequisites (cmake, clang
resource headers, kernel floor) and the systemd unit.

## Testing

```bash
cargo test --workspace
```

A `--all-features` run additionally enables optional surfaces
(`quic_native`, `xdp_userspace`, etc.). h2spec / Autobahn-style
conformance suites are opt-in and documented in `DEPLOYMENT.md`.

## Documentation

- `RUNBOOK.md` — operational procedures, every alert that can fire,
  triage matrix.
- `DEPLOYMENT.md` — systemd unit, capabilities, sysctls, build-time
  deps, XDP toolchain caveat.
- `METRICS.md` — every Prometheus family exported, label cardinality
  budget, scrape configuration.
- `CHANGELOG.md` — release-notes-format changelog.
- `CONFIG.md` — TOML schema.
- `SECURITY.md` — disclosure policy + threat model.

## License

GPL-3.0-only.
