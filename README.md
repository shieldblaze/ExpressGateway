# ExpressGateway

ExpressGateway is a memory-safe **L4 + L7 load balancer and reverse proxy**
written in Rust. It is built for teams that want first-class HTTP/3 and QUIC,
bounded-memory streaming across every HTTP version, and a security posture they
can audit — operated as a single binary (`expressgateway`) and one TOML file.
Reach for it when those properties matter more than a mature plugin ecosystem or
a dynamic control plane; if you need those, a mature incumbent is the better fit
(see [`docs/guide/comparison.md`](docs/guide/comparison.md)).

## The four things it does well

- **The full 9-cell HTTP matrix, streamed.** Any of HTTP/1.1, HTTP/2, HTTP/3 on
  the client side, translated to any of HTTP/1.1, HTTP/2, HTTP/3 on the backend
  side — nine front × back combinations, each streamed frame-by-frame with
  bounded memory (64 MiB caps, `413` past them; no whole-body buffering).
- **QUIC two ways.** **Mode A passthrough** routes QUIC flows by Connection ID
  **without decrypting** (TLS stays end-to-end client↔backend); **Mode B
  terminate** ends the client QUIC and re-originates a fresh upstream QUIC
  connection.
- **A hand-audited DoS-mitigation catalog** enforced on live listeners:
  Rapid-Reset, CONTINUATION flood, HPACK/QPACK bomb, SETTINGS/PING flood,
  zero-window stall, slowloris, slow-POST, request smuggling (CL.TE / TE.CL /
  H2-downgrade), and QUIC 0-RTT replay. The S38 security audit closed at
  **0 Critical, 0 High, 1 Medium (fixed), 7 Low, 4 Info** — see
  [`SECURITY.md`](SECURITY.md).
- **Panic-free libraries.** Every `crates/*/src/lib.rs` denies
  `unwrap`/`expect`/`panic`/`indexing_slicing` at the crate root and CI's
  `panic-freedom` job enforces it; the binary additionally installs a panic hook
  that bumps `panic_total` and logs before aborting.

## Capabilities at a glance

Beyond the four pillars, ExpressGateway ships:

- **L4 / XDP data plane** — an in-kernel XDP/eBPF TCP+UDP fast path; the compiled
  BPF ELF ships in-tree. **Off by default** (`[runtime].xdp_enabled`),
  single-kernel; validated live on Linux 7.0. See
  [`docs/guide/DEPLOYMENT.md`](docs/guide/DEPLOYMENT.md).
- **gRPC** over an H2 or H3 front (an H1 front cannot deliver `grpc-status`
  trailers — matches nginx).
- **WebSocket** over H1 `Upgrade` (RFC 6455, on by default), H3 extended-CONNECT
  (RFC 9220, opt-in), and H2 extended-CONNECT (RFC 8441, **gated off** by
  default).
- **TLS** via rustls + BoringSSL — TLS 1.2 + 1.3 by default (downgrade-safe),
  `tls13_only` opt-in; no server-side mTLS (intentional), upstream verification
  enforced.
- **HTTP/3 rollout** by advertising `Alt-Svc: h3` from an H1/H1s listener.
- **Conformance** — h2spec 147/147; h3spec passes with 12 named waivers.
- **Operations** — SIGHUP config hot reload, SIGUSR1 cert rotation, graceful
  drain on SIGTERM (lameduck + bounded drain; proactive per-protocol `GOAWAY` /
  `Connection: close` emission is in progress —
  [`docs/known-limitations.md`](docs/known-limitations.md)), per-IP and
  in-flight connection caps, Prometheus `/metrics` + Kubernetes probes,
  `LB_LOG_FORMAT` (json/text/plain).

For the full supported / gated / waived view, see
[`docs/guide/capabilities.md`](docs/guide/capabilities.md) and the matrix in
[`docs/features.md`](docs/features.md).

**A few things to know up front** (each links its canonical home):

- **Load balancing is round-robin today** for L7 HTTP and raw-TCP listeners,
  plus Maglev consistent-hashing by Connection ID for QUIC Mode A passthrough.
  Ten further algorithms are implemented in the `lb-balancer` library but are
  **not yet selectable from config** — [`docs/features.md`](docs/features.md)
  "Load balancing".
- **QUIC H3-terminate and Mode B forward to a single backend** (not
  load-balanced across a pool; Mode A passthrough is the exception, hashing
  flows across its pool) — [`docs/features.md`](docs/features.md) "Load
  balancing".
- **Health tracking is passive** and, in this build, **not yet wired into
  backend selection**; active probing is deferred —
  [`docs/known-limitations.md`](docs/known-limitations.md).
- **It is stateless.** Scale by running N independent instances behind any
  L4/L3 load balancer; `SO_REUSEPORT` lets a supervisor run a replacement
  process side-by-side for handover. There is **no built-in clustering or
  failover** — [`docs/guide/deployment-patterns.md`](docs/guide/deployment-patterns.md).
- **No response compression** (the gateway is pass-through for content encoding).

## Start here

- **New to ExpressGateway?** Read [`docs/guide/overview.md`](docs/guide/overview.md)
  — what it is, how a request flows, and when to use it.
- **Want to run it?** [`docs/guide/getting-started.md`](docs/guide/getting-started.md)
  — a container quickstart, a from-source build, and a second HTTPS/H2 walkthrough.
- **Evaluating it?** [`docs/guide/capabilities.md`](docs/guide/capabilities.md)
  (supported / gated / waived), [`docs/guide/comparison.md`](docs/guide/comparison.md)
  (vs Envoy/Traefik/HAProxy/nginx), and [`docs/guide/PERFORMANCE.md`](docs/guide/PERFORMANCE.md)
  (measured baseline).
- **How does it work?** [`docs/arch/overview.md`](docs/arch/overview.md) and the
  [architecture docs](docs/arch/).

## Building

```bash
cargo build --release -p lb --bin expressgateway
```

The produced artifact is `target/release/expressgateway`. The crate is named
`lb`; the binary it produces is `expressgateway` — always build with
`--bin expressgateway`. See [`docs/guide/DEPLOYMENT.md`](docs/guide/DEPLOYMENT.md)
for system-level prerequisites (cmake, clang resource headers, kernel floor) and
the systemd unit. To run it in a container instead, the
[getting-started guide](docs/guide/getting-started.md) leads with a Docker
quickstart.

## Quickstart

The binary takes the config path as a **positional argument** (there is **no
`--config` flag** — passing `--config foo.toml` will not work):

```bash
# Run with an explicit config:
expressgateway /etc/expressgateway/lb.toml

# Run with no argument → loads config/default.toml:
expressgateway
```

Pick a starting point from [`config/examples/`](config/examples/) (one per
deployment shape — TCP, TLS, H1, HTTPS/H2, gRPC, WebSocket, HTTP/3, QUIC
Mode A/B); each file's header comment shows the exact boot command. See
[`docs/guide/CONFIG.md`](docs/guide/CONFIG.md) for the full schema.

On a config parse or validation error the binary **exits non-zero and never
starts a partial listener set** — a misconfigured gateway does not boot half-up.

### Admin endpoints & metrics

Set `[observability].metrics_bind` (recommended `127.0.0.1:9090`) to bind the
admin HTTP listener:

| Endpoint | Purpose |
|----------|---------|
| `GET /metrics` | Prometheus text exposition (token-gated when `[admin].api_token_hash` is set). |
| `GET /livez`, `/healthz` | Liveness — 200 while the process is alive. |
| `GET /readyz` | Readiness — 503 during drain (lameduck signal for the orchestrator). |
| `GET /startupz` | 200 once boot completes; 503 during startup. |

The admin listener defaults to **loopback-only**; binding a non-loopback address
requires a bearer token (`[admin]`). Probes (`/livez` etc.) are intentionally
token-exempt for kubelet use; `/metrics` is gated. See
[`docs/guide/METRICS.md`](docs/guide/METRICS.md) and [`SECURITY.md`](SECURITY.md).

### Reload

`systemctl reload expressgateway` (SIGHUP) hot-reloads the swappable config
subset (backends, HTTP timeouts) without dropping connections; restart-required
changes are logged, never silently applied. SIGUSR1 rotates TLS certs; SIGTERM
starts a graceful drain. See [`docs/guide/CONFIG.md`](docs/guide/CONFIG.md)
"Reload semantics" and [`docs/guide/RUNBOOK.md`](docs/guide/RUNBOOK.md).

## Testing

```bash
cargo test --workspace
```

A `--all-features` run additionally enables optional surfaces (`quic_native`,
`xdp_userspace`, etc.). h2spec / Autobahn-style conformance suites are opt-in and
documented in [`docs/guide/DEPLOYMENT.md`](docs/guide/DEPLOYMENT.md).

## Documentation

**Guides** (`docs/guide/`) — learn and operate:

- [overview](docs/guide/overview.md) — what it is, how a request flows, when to use it.
- [getting-started](docs/guide/getting-started.md) — container + from-source quickstarts.
- [capabilities & limitations](docs/guide/capabilities.md) — the support matrix.
- [comparison](docs/guide/comparison.md) — vs Envoy / Traefik / HAProxy / nginx.
- [performance](docs/guide/PERFORMANCE.md) — the measured baseline and how to read it.
- [CONFIG](docs/guide/CONFIG.md) — TOML schema, reload semantics, worked examples.
- [cookbook](docs/guide/cookbook.md) — annotated, copy-paste config recipes for common setups.
- [DEPLOYMENT](docs/guide/DEPLOYMENT.md) — systemd unit, capabilities, sysctls, XDP toolchain.
- [deployment-patterns](docs/guide/deployment-patterns.md) — topology, scaling, handover.
- [RUNBOOK](docs/guide/RUNBOOK.md) — operational procedures, alerts, triage.
- [troubleshooting](docs/guide/troubleshooting.md) — common failures and their fixes.
- [METRICS](docs/guide/METRICS.md) — every Prometheus family, cardinality budget, scraping.

**Reference & architecture:**

- [`docs/features.md`](docs/features.md) — protocol matrix and supported / gated / waived set.
- [`docs/known-limitations.md`](docs/known-limitations.md) — bounded, documented operator-facing constraints.
- [`docs/glossary.md`](docs/glossary.md) — Mode A/B, 9-cell, CID, lameduck, and the rest of the jargon.
- [`docs/arch/`](docs/arch/) — developer/architecture docs: [overview](docs/arch/overview.md),
  [protocol model](docs/arch/protocol-model.md), [QUIC modes](docs/arch/quic-modes.md),
  [backpressure](docs/arch/backpressure.md),
  [security & conformance](docs/arch/security-and-conformance.md).
- [`docs/architecture.md`](docs/architecture.md) — crate map and data-plane internals.
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — build, test, run the gates, project conventions.
- [`SECURITY.md`](SECURITY.md) — threat model, defenses, S38 audit posture, disclosure policy.
- [`CHANGELOG.md`](CHANGELOG.md) — release-notes-format changelog.

## License

GPL-3.0-only.
