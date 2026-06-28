# Getting Started

This page gets you from nothing to a running load balancer serving a real
request, two ways:

- **[Container quickstart](#container-quickstart)** — build the image and serve a
  request through it. Fewest moving parts; no Rust toolchain on your machine.
- **[From source](#from-source)** — build the binary directly, then a second
  walkthrough adds HTTPS + HTTP/2.

New to the project? Read [`overview.md`](overview.md) first for what
ExpressGateway is and where it fits.

---

## Container quickstart

The repo ships a `Dockerfile` and a CI gate (`scripts/ci/docker-smoke.sh`) that
builds the image, runs a backend and the gateway on a private network, and curls
a real request through it on **every build**. The steps below are that same
recipe, run by hand. (We don't run Docker on the docs box — the commands and
output here mirror the CI gate; build times and dates on your machine will
differ.)

**Prerequisites:** Docker, and a checkout of this repo (`cd` into it). That's all
— no Rust toolchain needed.

### 1. Build the image

```bash
docker build -f docker/Dockerfile -t expressgateway:local .
```

> The **first** build compiles BoringSSL from source (via `quiche`), which is
> slow — budget several minutes. It is a one-time cost; later builds reuse the
> cached layers.

### 2. Create a private network

The gateway resolves its backend by hostname, so put both containers on one
user-defined network:

```bash
docker network create eg-net
```

### 3. Start a throwaway backend

`hashicorp/http-echo` answers every request with a fixed body and `200`. The
`--network-alias backend` is the hostname the gateway's config resolves:

```bash
docker run -d --name eg-backend --network eg-net --network-alias backend \
  hashicorp/http-echo:latest -listen=":8080" -text="hello from http-echo"
```

### 4. Start the gateway

Mount the bundled smoke config over the image's default config path (the binary
reads it as its positional argument) and publish the plaintext listener to your
host:

```bash
docker run -d --name eg-gw --network eg-net \
  -p 127.0.0.1:8080:8080 \
  -v "$PWD/docker/smoke/gateway.toml:/etc/expressgateway/config.toml:ro" \
  expressgateway:local
```

That config (`docker/smoke/gateway.toml`) defines one HTTP/1.1 listener on
`0.0.0.0:8080` proxying to `backend:8080`.

### 5. Send a request through it

```bash
curl -v http://127.0.0.1:8080/
```

Expected (your date/header set will vary — the status line and body are the
payoff):

```http
> GET / HTTP/1.1
> Host: 127.0.0.1:8080
> Accept: */*
>
< HTTP/1.1 200 OK
< content-type: text/plain; charset=utf-8
< content-length: 21
< x-app-name: http-echo
<
hello from http-echo
```

A `200` with the backend's body means the request went **client → gateway →
backend → client**. That is exactly what the `docker-smoke` CI gate asserts.

### 6. Clean up

```bash
docker rm -f eg-gw eg-backend && docker network rm eg-net
```

> **Scraping metrics from a container.** The smoke config binds the admin
> listener (`/metrics`, `/readyz`) on container-loopback, so it isn't reachable
> from your host as-is. To scrape Prometheus in a real deployment, bind the admin
> listener on a routable address behind a bearer token — see
> [`METRICS.md`](METRICS.md) and [`../../SECURITY.md`](../../SECURITY.md). The
> [from-source walkthrough](#5-check-metrics--health) below shows the `/metrics`
> payoff directly on loopback.

---

## From source

This builds the binary and serves an HTTP/1.1 request, then a second walkthrough
adds HTTPS + HTTP/2.

### Prerequisites

**Rust toolchain.** The workspace pins Rust **1.88** (`rust-toolchain.toml`,
which also installs `rustfmt` + `clippy`). With `rustup` installed, the pinned
toolchain is selected automatically inside the repo.

**Build-time system dependencies** (because `quiche` links BoringSSL, whose build
is driven by cmake — full detail in [`DEPLOYMENT.md`](DEPLOYMENT.md)):

- `cmake` (≥ 3.20)
- a C/C++ toolchain (`build-essential` on Debian/Ubuntu)
- libclang resource headers (`boring-sys`'s bindgen needs `stddef.h`)
- ~20 GB of scratch disk for the first build (BoringSSL + quiche + release
  artifacts)

> The first build compiles BoringSSL from source (several minutes cold);
> subsequent builds cache it.

**Kernel floor** is only relevant if you enable the optional XDP data plane (off
by default): Linux ≥ 5.15 effective floor. The quickstart below does **not** use
XDP, so any modern Linux (or macOS/BSD for dev builds) works.

### 1. Build

```bash
cargo build --release -p lb --bin expressgateway
```

The produced artifact is **`target/release/expressgateway`**. The crate is named
`lb`; the binary it produces is `expressgateway` — always build with
`--bin expressgateway`.

### 2. Pick a config

The repo ships nine worked examples under
[`config/examples/`](../../config/examples/), one per deployment shape. The
simplest L7 starting point is **`h1.toml`** — plain HTTP/1.1 over TCP:

```toml
# config/examples/h1.toml (abridged)
[[listeners]]
address  = "0.0.0.0:8080"
protocol = "h1"

[[listeners.backends]]
address  = "127.0.0.1:3000"
protocol = "h1"
```

This binds an HTTP/1.1 listener on `0.0.0.0:8080` and proxies to one backend at
`127.0.0.1:3000`. **Edit the backend `address`** to your real upstream; add more
`[[listeners.backends]]` blocks for a pool. Selection across a pool is
**round-robin** (the per-backend `weight` field is accepted but not enforced in
this build — round-robin ignores it; see [`CONFIG.md`](CONFIG.md)).

### 3. Run

The binary takes the config path as a **positional argument** — there is **no
`--config` flag** (passing `--config foo.toml` will not work):

```bash
target/release/expressgateway config/examples/h1.toml
```

With **no argument**, it loads `config/default.toml` instead.

If the file fails to parse or validate, the binary prints the error chain to
stderr and **exits non-zero without starting any listener** — a misconfigured
gateway never boots half-up. Strict parsing also means an unknown or misspelled
key is a hard error, not a silent drop.

For a quick end-to-end test without a real upstream, run any HTTP server on the
backend port first:

```bash
# Terminal 1 — a throwaway backend on :3000
python3 -m http.server 3000

# Terminal 2 — the gateway
target/release/expressgateway config/examples/h1.toml
```

### 4. Send a request

```bash
curl -v http://127.0.0.1:8080/
```

You should see the `HTTP/1.1 200 OK` status line, the backend's headers, and the
proxied body. The response is streamed back frame-by-frame — the gateway never
buffers a whole body.

### 5. Check metrics & health

The admin endpoints are off until you bind them. Add an `[observability]` block
(loopback bind recommended):

```toml
[observability]
metrics_bind = "127.0.0.1:9090"
```

Restart, send a request or two through `:8080`, then scrape:

```bash
curl -s http://127.0.0.1:9090/metrics
```

The output is Prometheus text format. After one proxied request you'll see (HELP
lines omitted for brevity):

```text
# TYPE connections_total counter
connections_total 1
# TYPE http_requests_total counter
http_requests_total{version="h1",status_class="2xx"} 1
# TYPE http_request_duration_seconds histogram
http_request_duration_seconds_count{version="h1"} 1
```

The probes are plain HTTP:

```bash
curl -s http://127.0.0.1:9090/livez      # 200 while alive
curl -s http://127.0.0.1:9090/readyz     # 503 during drain
curl -s http://127.0.0.1:9090/startupz   # 200 once boot completes
```

The admin listener defaults to **loopback-only**; binding a non-loopback address
requires a bearer token (`[admin]`). `/metrics` is token-gated when
`[admin].api_token_hash` is set; the probes are intentionally token-exempt for
kubelet use. Full catalog: [`METRICS.md`](METRICS.md).

### 6. Reload without dropping connections

After editing the **swappable** subset of the config (the backend pool, the
per-listener HTTP timeouts), apply it live with SIGHUP:

```bash
kill -HUP <pid>            # or: systemctl reload expressgateway
```

Restart-required changes (e.g. `protocol`, `address`, TLS/QUIC blocks) are
**logged and counted, never silently applied**. `SIGUSR1` rotates TLS certs;
`SIGTERM` starts a graceful drain. See [`CONFIG.md`](CONFIG.md) "Reload
semantics".

---

## Second walkthrough: HTTPS + HTTP/2

HTTP/2 is served on an **`h1s`** (HTTPS) listener: ALPN advertises `h2` first,
then `http/1.1`, so an HTTP/2 client negotiates H2 and an older client falls back
to HTTP/1.1 on the same port. There is no separate `"h2"` listener protocol.

### 1. Generate a self-signed certificate

For local testing only — use a real certificate in production:

```bash
openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout key.pem -out cert.pem -days 365 -subj "/CN=localhost"
chmod 600 key.pem
```

### 2. Point `h1s.toml` at the cert and a backend

Start from [`config/examples/h1s.toml`](../../config/examples/h1s.toml) and set
the TLS paths to the files you just generated (absolute paths are safest):

```toml
[[listeners]]
address  = "0.0.0.0:8443"
protocol = "h1s"

[listeners.tls]
cert_path = "/absolute/path/to/cert.pem"
key_path  = "/absolute/path/to/key.pem"

[[listeners.backends]]
address  = "127.0.0.1:3000"
protocol = "h1"
```

### 3. Run a backend and the gateway

```bash
# Terminal 1 — throwaway backend
python3 -m http.server 3000

# Terminal 2 — the gateway
target/release/expressgateway config/examples/h1s.toml
```

### 4. Request it over HTTP/2

`--http2` asks curl to negotiate H2 via ALPN; `-k` accepts the self-signed cert:

```bash
curl --http2 -vk https://127.0.0.1:8443/
```

Expected (the load-bearing lines):

```http
* ALPN: curl offers h2,http/1.1
* ALPN: server accepted h2
* using HTTP/2
> GET / HTTP/2
> Host: 127.0.0.1:8443
>
< HTTP/2 200
```

`ALPN: server accepted h2` then `HTTP/2 200` confirms the H2 front. The backend
here speaks HTTP/1.1, so this is the **H2 → H1** cell of the matrix in action.

---

## Common first errors

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| `cannot read config file: --config` | You passed `--config foo.toml`. There is **no** `--config` flag. | Pass the path positionally: `expressgateway foo.toml`. |
| Boot exits non-zero with `unknown field` / a validation error | Strict parsing rejected a misspelled or unsupported key. | Fix the key; the gateway never boots half-configured. See [`CONFIG.md`](CONFIG.md). |
| `Address already in use` on boot | Another process holds the listener port. | Free the port or change the listener `address`. |
| `502` from the gateway | The backend is down or its `address`/DNS doesn't resolve. | Confirm the backend is up and reachable from the gateway. |
| First container/build is very slow | BoringSSL compiles from source. | One-time cost; later builds reuse the cache. |

More symptoms (XDP attach, cert reload didn't take, H3 not negotiated) are in
[`troubleshooting.md`](troubleshooting.md).

## Next steps

- **Production deployment** — systemd unit, Linux capabilities, sysctls, the XDP
  toolchain caveat: [`DEPLOYMENT.md`](DEPLOYMENT.md).
- **Topology & scaling** — running N stateless instances, handover:
  [`deployment-patterns.md`](deployment-patterns.md).
- **Other listener shapes** — gRPC, WebSocket, HTTP/3, QUIC Mode A/B: pick the
  matching `config/examples/*.toml` and see [`CONFIG.md`](CONFIG.md).
- **What's supported / gated / deferred** — [`capabilities.md`](capabilities.md).
- **Operating it** — alerts, triage, graceful drain: [`RUNBOOK.md`](RUNBOOK.md).
