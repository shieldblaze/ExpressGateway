# Getting Started

This walks you from a fresh checkout to a running L7 HTTP/1.1 load balancer
serving traffic, in the simplest possible shape. For production deployment
(systemd unit, capabilities, sysctls, XDP), continue to
[`DEPLOYMENT.md`](DEPLOYMENT.md) afterward; for the full config schema, see
[`CONFIG.md`](CONFIG.md).

New to the project? Read [`overview.md`](overview.md) first for what
ExpressGateway is and where it fits.

## Prerequisites

**Rust toolchain.** The workspace pins Rust **1.88** (`rust-toolchain.toml`,
which also installs `rustfmt` + `clippy`). With `rustup` installed, the pinned
toolchain is selected automatically inside the repo.

**Build-time system dependencies** (because `quiche` links BoringSSL, whose
build is driven by cmake — full detail in [`DEPLOYMENT.md`](DEPLOYMENT.md)):

- `cmake` (≥ 3.20)
- a C/C++ toolchain (`build-essential` on Debian/Ubuntu)
- libclang resource headers (`boring-sys`'s bindgen needs `stddef.h`; on
  Ubuntu 24.04 the workspace's `.cargo/config.toml` already points
  `BINDGEN_EXTRA_CLANG_ARGS` at the gcc include dir — see DEPLOYMENT.md)
- ~20 GB of scratch disk for the first build (BoringSSL + quiche + release
  artifacts)

> The first build compiles BoringSSL from source (~6–8 minutes cold);
> subsequent builds cache it.

**Kernel floor** is only relevant if you enable the optional XDP data plane
(off by default): Linux ≥ 5.15 effective floor, validated on 5.15 / 6.1 / 6.6
LTS and live on 7.0. The quickstart below does **not** use XDP, so any modern
Linux (or macOS/BSD for dev builds) works. See [`DEPLOYMENT.md`](DEPLOYMENT.md)
"Kernel floor".

## 1. Build

```bash
cargo build --release -p lb --bin expressgateway
```

The produced artifact is **`target/release/expressgateway`**. (The crate is
named `lb`; the binary it produces is `expressgateway` — always build with
`--bin expressgateway`.)

## 2. Pick a config

The repo ships nine worked examples under
[`config/examples/`](../../config/examples/), one per deployment shape. Each
file's header comment shows its exact boot command. The simplest L7 starting
point is **`h1.toml`** — plain HTTP/1.1 over TCP:

```toml
# config/examples/h1.toml (abridged)
[[listeners]]
address = "0.0.0.0:8080"
protocol = "h1"

[[listeners.backends]]
address  = "127.0.0.1:3000"
protocol = "h1"
weight   = 1
```

This binds an HTTP/1.1 listener on `0.0.0.0:8080` and proxies to a single
backend at `127.0.0.1:3000`. **Edit the backend `address`** to point at your
real upstream (add more `[[listeners.backends]]` blocks for a pool — selection
is round-robin by default).

The full set of examples (`tcp`, `tls`, `h1`, `h1s`, `h1s-grpc`,
`h1s-websocket`, `quic-h3`, `quic-mode-b`, `passthrough-mode-a`) is tabulated
in [`CONFIG.md`](CONFIG.md) "Worked examples".

## 3. Run

The binary takes the config path as a **positional argument** — there is **no
`--config` flag** (passing `--config foo.toml` will not work):

```bash
target/release/expressgateway config/examples/h1.toml
```

With **no argument**, it loads `config/default.toml` (a minimal single
plain-TCP listener) instead.

**Fail-fast on bad config.** If the file fails to parse or validate, the binary
prints the error chain to stderr and **exits non-zero without starting any
listener** — a misconfigured gateway never boots half-up. (Strict parsing also
means an unknown or misspelled key is a hard error, not a silent drop — see
[`CONFIG.md`](CONFIG.md) "Strict parsing".)

For a quick end-to-end test without a real upstream, run any HTTP server on the
backend port first, e.g.:

```bash
# In one terminal — a throwaway backend on :3000
python3 -m http.server 3000

# In another — the gateway (after editing h1.toml's backend to 127.0.0.1:3000)
target/release/expressgateway config/examples/h1.toml
```

## 4. Send a request

```bash
curl -v http://127.0.0.1:8080/
```

The request is proxied to the backend and the response streamed back. (`-v`
shows the `HTTP/1.1` status line and headers.)

## 5. Check metrics & health

The admin endpoints are off until you bind them. Add an `[observability]` block
to your config (recommended loopback bind) — see [`CONFIG.md`](CONFIG.md)
"`[observability]`":

```toml
[observability]
metrics_bind = "127.0.0.1:9090"
```

Then:

```bash
curl -s http://127.0.0.1:9090/metrics      # Prometheus text exposition
curl -s http://127.0.0.1:9090/livez        # liveness — 200 while alive
curl -s http://127.0.0.1:9090/readyz       # readiness — 503 during drain
curl -s http://127.0.0.1:9090/startupz     # 200 once boot completes
```

The admin listener defaults to **loopback-only**; binding a non-loopback
address requires a bearer token (`[admin]`). `/metrics` is token-gated when
`[admin].api_token_hash` is set; the liveness/readiness probes are
intentionally token-exempt for kubelet use. Full endpoint and metric catalog:
[`METRICS.md`](METRICS.md) and [`../../SECURITY.md`](../../SECURITY.md).

## 6. Reload without dropping connections

After editing the **swappable** subset of the config (the backend pool, the
per-listener HTTP timeouts), apply it live with SIGHUP:

```bash
kill -HUP <pid>            # or: systemctl reload expressgateway
```

Restart-required changes (e.g. `protocol`, `address`, TLS/QUIC blocks) are
**logged and counted, never silently applied** — the live config keeps serving
until you restart. `SIGUSR1` rotates TLS certs; `SIGTERM` starts a graceful
drain. See [`CONFIG.md`](CONFIG.md) "Reload semantics".

## Next steps

- **Production deployment** — systemd unit, Linux capabilities, sysctls, rlimits,
  the XDP toolchain caveat: [`DEPLOYMENT.md`](DEPLOYMENT.md).
- **Other listener shapes** — HTTPS (H1+H2), gRPC, WebSocket, HTTP/3, QUIC Mode
  A/B: pick the matching `config/examples/*.toml` and see [`CONFIG.md`](CONFIG.md).
- **What's supported / gated / deferred** — [`capabilities.md`](capabilities.md).
- **Operating it** — alerts, triage, graceful drain: [`RUNBOOK.md`](RUNBOOK.md).
