# ExpressGateway

A globally distributed L4 + L7 load balancer written in Rust, featuring:

- **L7 data plane**: HTTP/1.1, HTTP/2, HTTP/3 — the full **9-cell
  front × back protocol matrix** (front {H1, H2, H3} × back {H1, H2, H3}),
  streamed frame-by-frame with bounded memory (no whole-body buffering).
  See [`docs/features.md`](docs/features.md) for the matrix and what is
  supported / gated / waived.
- **L4 data plane**: XDP/eBPF-based TCP and UDP load balancing. The
  compiled BPF ELF (`crates/lb-l4-xdp/src/lb_xdp.bin`) ships in-tree and
  the userspace loader attaches it; validated live on Linux 7.0 (native
  ENA `xdpdrv` attach). Single-kernel — see `docs/guide/DEPLOYMENT.md`.
- **QUIC-native proxying** via quiche, in two modes: **Mode A
  passthrough** (route by Connection ID, no decryption) and **Mode B
  terminate** (re-originate a fresh upstream QUIC connection).
- **Native gRPC** proxy with full streaming support over an **H2 or H3
  front** (an H1 front cannot deliver `grpc-status` trailers — see
  [Known Limitations](docs/known-limitations.md)).
- **WebSocket** over **H1 `Upgrade`** (RFC 6455, on by default), **H2
  extended-CONNECT** (RFC 8441, **gated OFF by default** — CF-S27-2), and
  **H3 extended-CONNECT** (RFC 9220, opt-in).
- **TLS** via rustls + BoringSSL (TLS 1.2 + 1.3 by default, downgrade-safe;
  `tls13_only` opt-in; no server-side mTLS — see `SECURITY.md`).
- **Conformance**: h2spec 147/147; h3spec passes with 12 named waivers
  (`CF-QUICHE-UPGRADE`, `scripts/ci/h3spec-check.sh`).
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
- **Graceful drain on `SIGTERM`**: lameduck `/readyz` flip → settle →
  cancellation-token cancel → bounded wait (`runtime.drain_timeout_ms`,
  default 10 s) → `shutdown_aborted_connections_total` bump on overflow.
  `SO_REUSEPORT` is set on listening sockets so a supervisor can run a
  replacement process side-by-side during a manual handover; the binary
  does not itself transfer FDs between processes (deferred). For the full
  operator-driven restart procedure see `docs/guide/RUNBOOK.md` "Drain (graceful
  shutdown)".
- **Panic-free libraries**: every `crates/*/src/lib.rs` carries
  `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, ...)]`;
  CI's `panic-freedom` job enforces it. The binary (`crates/lb`) carries
  the same deny lints and installs a `std::panic::set_hook` that bumps
  `panic_total` and emits a structured `tracing::error!` before the
  process continues. The `expect`/`unwrap` calls remaining in source
  live inside `#[cfg(test)] mod tests { ... }` blocks.

## Building

```bash
cargo build --release -p lb --bin expressgateway
```

The produced artifact is `target/release/expressgateway`. See
`docs/guide/DEPLOYMENT.md` for system-level prerequisites (cmake, clang
resource headers, kernel floor) and the systemd unit.

## Quickstart

The binary takes the config path as a **positional argument** (there is
**no `--config` flag** — passing `--config foo.toml` will not work):

```bash
# Run with an explicit config:
expressgateway /etc/expressgateway/lb.toml

# Run with no argument → loads config/default.toml:
expressgateway
```

Pick a starting point from [`config/examples/`](config/examples/) (one per
deployment shape — TCP, TLS, H1, HTTPS/H2, gRPC, WebSocket, HTTP/3, QUIC
Mode A/B); each file's header comment shows the exact boot command. See
`docs/guide/CONFIG.md` for the full schema.

On a config parse or validation error the binary **exits non-zero and
never starts a partial listener set** — a misconfigured gateway does not
boot half-up.

### Admin endpoints & metrics

Set `[observability].metrics_bind` (recommended `127.0.0.1:9090`) to bind
the admin HTTP listener:

| Endpoint | Purpose |
|----------|---------|
| `GET /metrics` | Prometheus text exposition (token-gated when `[admin].api_token_hash` is set). |
| `GET /livez`, `/healthz` | Liveness — 200 while the process is alive. |
| `GET /readyz` | Readiness — 503 during drain (lameduck signal for the orchestrator). |
| `GET /startupz` | 200 once boot completes; 503 during startup. |

The admin listener defaults to **loopback-only**; binding a non-loopback
address requires a bearer token (`[admin]`). Probes (`/livez` etc.) are
intentionally token-exempt for kubelet use; `/metrics` is gated. See
`docs/guide/METRICS.md` and `SECURITY.md`.

### Reload

`systemctl reload expressgateway` (SIGHUP) hot-reloads the swappable config
subset (backends, HTTP timeouts) without dropping connections;
restart-required changes are logged, never silently applied. SIGUSR1
rotates TLS certs. See `docs/guide/CONFIG.md` "Reload semantics" and `docs/guide/RUNBOOK.md`.

## Testing

```bash
cargo test --workspace
```

A `--all-features` run additionally enables optional surfaces
(`quic_native`, `xdp_userspace`, etc.). h2spec / Autobahn-style
conformance suites are opt-in and documented in `docs/guide/DEPLOYMENT.md`.

## Documentation

- `docs/guide/RUNBOOK.md` — operational procedures, every alert that can fire,
  triage matrix.
- `docs/guide/DEPLOYMENT.md` — systemd unit, capabilities, sysctls, build-time
  deps, XDP toolchain caveat.
- `docs/guide/METRICS.md` — every Prometheus family exported, label cardinality
  budget, scrape configuration.
- `CHANGELOG.md` — release-notes-format changelog.
- `docs/guide/CONFIG.md` — TOML schema, reload semantics, worked examples.
- `SECURITY.md` — threat model, defenses, S38 audit posture,
  disclosure policy.
- `docs/features.md` — protocol matrix and supported / gated / waived
  feature set.
- `docs/known-limitations.md` — bounded, documented operator-facing
  constraints (WS-H2 gating, gRPC front requirement, named waivers, …).
- `docs/architecture.md` — crate graph and data-plane internals.

## License

GPL-3.0-only.
