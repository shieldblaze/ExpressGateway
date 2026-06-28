# Configuration Cookbook

Task-oriented recipes that assemble configuration knobs toward a goal. Each
recipe states the task, shows a **minimal working config** first, then layers
on options with the *why* of each directive, and ends with a command you can
run to **verify it works**. The [Combined Configuration Example](#combined-configuration-example)
at the end stitches the recipes into one production config you can copy and
adapt.

This page is the "how do I build X" companion to [`CONFIG.md`](CONFIG.md),
which is the per-knob reference (every key, type, range, default, reload
class). When a recipe shows a knob, look it up there for the full contract.

A note on backend selection used throughout: a listener with more than one
`[[listeners.backends]]` entry is balanced **round-robin**. A per-backend
`weight` key is accepted by the schema but is **not yet enforced** in this
build — round-robin ignores it — so the recipes below omit it. See
[`CONFIG.md`](CONFIG.md) "`[[listeners.backends]]`".

All TLS recipes assume a cert/key pair at `/etc/expressgateway/tls/`. The
private key must be mode `0600` (group/other-readable keys are rejected at
boot). Use `-k` with `curl` in the verify steps only when testing against a
self-signed cert.

---

## Recipe 1 — Production HTTPS with HTTP/2

**Task:** terminate TLS and serve both HTTP/1.1 and HTTP/2 on one port,
proxying to a backend pool.

HTTP/2 is served by the **`h1s`** listener: its TLS handshake advertises
`h2` (preferred) and `http/1.1` (fallback) via [ALPN](../glossary.md), so an
H2 client gets HTTP/2 and an older client gets HTTP/1.1 on the same socket.
There is no separate `h2` listener protocol.

```toml
[[listeners]]
address  = "0.0.0.0:443"
protocol = "h1s"            # HTTP/1.1 + HTTP/2 over TLS (ALPN-negotiated)

[listeners.tls]
cert_path = "/etc/expressgateway/tls/cert.pem"
key_path  = "/etc/expressgateway/tls/key.pem"

[[listeners.backends]]
address  = "10.0.0.10:8080"
protocol = "h1"             # upstream wire protocol; h1/tcp => HTTP/1.1

[[listeners.backends]]
address  = "10.0.0.11:8080"
protocol = "h1"
```

Two backends means round-robin across the pool. The backend `protocol` is
an independent axis from the front: here the client speaks H2 and the
upstream speaks HTTP/1.1 — the gateway translates between them.

**Layered options.**

Restrict to TLS 1.3 (e.g. for a PCI-DSS-style requirement). This is a
process-wide policy, so it applies to every TLS listener:

```toml
[runtime.tls]
tls13_only = true
```

Expose metrics and health probes on a loopback admin listener (you almost
always want this in production — see [`observability.md`](observability.md)):

```toml
[observability]
metrics_bind = "127.0.0.1:9090"
```

Give a streaming listener a longer graceful-drain budget than the 10 s
default so long-lived responses aren't truncated on restart:

```toml
[[listeners]]
address          = "0.0.0.0:443"
protocol         = "h1s"
drain_timeout_ms = 60000     # allow one full long-poll/SSE cycle to finish
```

The full drain procedure and how to size this budget per workload live in
[`RUNBOOK.md`](RUNBOOK.md) "Tuning the drain budget".

**Verify.**

```bash
curl -vk --http2 https://127.0.0.1/ 2>&1 | grep -E 'ALPN|HTTP/2'
# * ALPN: server accepted h2
# < HTTP/2 200
```

`ALPN: server accepted h2` confirms HTTP/2 negotiated; the `HTTP/2 200`
status line confirms the request reached the backend and came back.

---

## Recipe 2 — Terminating gRPC

**Task:** proxy gRPC clients to a gRPC backend, with a server-side deadline
clamp and an edge-served health check.

gRPC carries its status in HTTP **trailers**, which only an HTTP/2 or
HTTP/3 front can deliver on a streamed response. So a gRPC front is an
`h1s` (H2) listener, and the backend speaks `h2`. (An HTTP/1.1 front cannot
return `grpc-status` — see [`known-limitations.md`](../known-limitations.md)
"gRPC requires an HTTP/2 or HTTP/3 front".)

```toml
[[listeners]]
address  = "0.0.0.0:443"
protocol = "h1s"            # H2 front via ALPN — required for gRPC trailers

[listeners.tls]
cert_path = "/etc/expressgateway/tls/cert.pem"
key_path  = "/etc/expressgateway/tls/key.pem"

[listeners.grpc]
enabled              = true
max_deadline_seconds = 300   # clamp any client grpc-timeout down to <= 5 min
health_synthesized   = true  # answer /grpc.health.v1.Health/Check at the edge

[[listeners.backends]]
address  = "10.0.0.20:50051"
protocol = "h2"             # gRPC backend MUST be h2 (or h3)
```

Why each `[listeners.grpc]` directive:

- **`max_deadline_seconds`** caps the effective deadline. A client can ask
  for any `grpc-timeout`; this clamps an over-long (or missing-and-defaulted)
  deadline so a single call cannot pin a backend stream open indefinitely.
- **`health_synthesized`** makes the gateway answer
  `/grpc.health.v1.Health/Check` itself with `SERVING`, so an L4 health
  checker or service mesh gets a fast local answer without a round-trip to
  the backend.

**Layered option.** To roll gRPC over HTTP/3 as well, add a `quic` listener
with an `h3` (or `h2`) backend — the trailer guarantee holds on H3 too.
Combine it with [Recipe 3](#recipe-3--rolling-out-http3) to advertise H3
from this H2 front.

**Verify** (with [`grpcurl`](https://github.com/fullstorydev/grpcurl)):

```bash
grpcurl -insecure 127.0.0.1:443 grpc.health.v1.Health/Check
# {
#   "status": "SERVING"
# }
```

A `SERVING` response proves the H2/ALPN path and the synthesized health
endpoint. Then call a real method on your service to confirm end-to-end
streaming and that `grpc-status: 0` trailers arrive.

---

## Recipe 3 — Rolling out HTTP/3

**Task:** add HTTP/3 to an existing HTTPS endpoint without breaking H1/H2
clients, by advertising H3 over `Alt-Svc`.

HTTP/3 runs over QUIC (UDP). Clients don't start with H3 — they connect
over your existing TCP HTTPS endpoint, see an `Alt-Svc: h3=...` header
telling them an H3 endpoint is available, and upgrade on a later request.
So a rollout needs **two listeners**: the existing `h1s` (TCP) listener,
now advertising H3, and a `quic` (UDP) listener that actually serves it.
Both can bind the **same port** — TCP/443 and UDP/443 are distinct sockets.

```toml
# 1) Existing HTTPS front (TCP) — now advertises H3 to clients.
[[listeners]]
address  = "0.0.0.0:443"
protocol = "h1s"

[listeners.tls]
cert_path = "/etc/expressgateway/tls/cert.pem"
key_path  = "/etc/expressgateway/tls/key.pem"

[listeners.alt_svc]
h3_port = 443               # emit `Alt-Svc: h3=":443"; ma=3600` on responses
# max_age = 3600            # default; the ma= advertisement lifetime in seconds

[[listeners.backends]]
address  = "10.0.0.10:8080"
protocol = "h1"

# 2) HTTP/3 front (QUIC over UDP, same port) — serves the upgraded clients.
[[listeners]]
address  = "0.0.0.0:443"
protocol = "quic"

[listeners.quic]
cert_path         = "/etc/expressgateway/tls/cert.pem"
key_path          = "/etc/expressgateway/tls/key.pem"
retry_secret_path = "/etc/expressgateway/quic/retry.secret"  # minted 0600 if absent

[[listeners.backends]]
address  = "10.0.0.10:8080"
protocol = "h1"             # H3 front -> HTTP/1.1 backend; the gateway translates
```

Why this works: `[listeners.alt_svc]` adds the advertisement to the TCP
listener's responses; the `quic` listener terminates client QUIC and speaks
HTTP/3 (`quic` defaults to H3-terminate mode). A client that can't do H3
simply never upgrades and keeps using H1/H2 — the rollout is safe by
construction. Make sure your network path allows **inbound UDP/443**; that
is the most common reason H3 silently fails to negotiate.

**Verify.** Check the advertisement is emitted by the TCP front:

```bash
curl -sIk https://127.0.0.1:443/ | grep -i alt-svc
# alt-svc: h3=":443"; ma=3600
```

If your `curl` is built with HTTP/3 support, confirm the H3 path directly:

```bash
curl -vk --http3 https://127.0.0.1:443/ 2>&1 | grep -i 'HTTP/3'
# < HTTP/3 200
```

---

## Recipe 4 — DoS hardening

**Task:** tighten the resource-limit envelope on an internet-facing
listener.

Start here: the protocol-level flood and smuggling defenses are **on by
default** and need no configuration — Rapid-Reset, CONTINUATION flood,
HPACK/QPACK decompression bombs, SETTINGS/PING floods, zero-window stalls,
and request-smuggling (CL.TE / TE.CL / H2-downgrade) detection are always
enforced on live listeners. See [`SECURITY.md`](../../SECURITY.md) and
[`features.md`](../features.md) "Security defenses". What you tune below is
the **resource budget** — how many connections, and how slow a client may
be before it's reaped.

```toml
[runtime]
per_ip_connection_cap    = 256     # cap concurrent connections per source IP
max_inflight_connections = 32768   # global in-flight ceiling, per listener

[runtime.watchdog]
header_deadline_ms    = 5000       # slowloris: finish request headers within 5 s
body_progress_min_bps = 64         # slow-POST: body must move >= 64 B/s
sweep_interval_ms     = 1000       # how often the stalled-connection reaper runs

[[listeners]]
address  = "0.0.0.0:443"
protocol = "h1s"

[listeners.tls]
cert_path = "/etc/expressgateway/tls/cert.pem"
key_path  = "/etc/expressgateway/tls/key.pem"

[[listeners.backends]]
address  = "10.0.0.10:8080"
protocol = "h1"
```

Why each knob:

- **`per_ip_connection_cap`** bounds how much of the connection table a
  single source IP can hold, so one client can't crowd out the rest. The
  default is 1024; lower it for a public endpoint with many distinct
  clients, raise it if a legitimate upstream NAT fronts many users.
- **`max_inflight_connections`** is the per-listener admission ceiling (a
  semaphore); beyond it, the listener sheds new accepts rather than
  unboundedly fanning out work.
- **`[runtime.watchdog]`** is the slowloris / slow-POST floor:
  `header_deadline_ms` kills a client that dribbles request headers, and
  `body_progress_min_bps` kills one that dribbles a request body below the
  floor rate. `sweep_interval_ms` is how often the reaper checks.

**Layered option.** Reject any `Transfer-Encoding` codec other than
`chunked` outright (a stricter smuggling posture for an H1 edge):

```toml
[security]
strict_te = true
```

Several of these knobs are **foot-guns when set to `0`/`false`** — that
*disables* the defense. The disable-values and what they cost are tabulated
in [`CONFIG.md`](CONFIG.md) "Foot-gun knobs".

**Verify.** Confirm the gateway booted with your limits (a misconfigured
value is rejected at boot, so a running process already proves the ranges
are valid), then watch the live accept/inflight behavior under load through
the alerts in [`RUNBOOK.md`](RUNBOOK.md) — `LbAcceptSaturation`,
`LbAcceptShed`, and `LbConnectionsInflight` are the relevant ones.

---

## Recipe 5 — QUIC Mode A passthrough

**Task:** front a fleet of QUIC/HTTP/3 backends at L4 **without decrypting**
traffic, keeping TLS end-to-end between client and backend.

Mode A is a top-level `[passthrough]` block — a parallel datapath, not a
`[[listeners]]` variant, so a passthrough-only config needs no
`[[listeners]]` at all. The gateway routes each QUIC flow to a backend by
**hashing its Connection ID** (consistent hashing, so a flow always lands
on the same backend) and forwards UDP packets without terminating TLS. The
gateway holds no TLS state. See [`arch/quic-modes.md`](../arch/quic-modes.md).

```toml
[passthrough]
bind_addr         = "0.0.0.0:443"
backends          = ["10.0.0.10:4443", "10.0.0.11:4443"]   # QUIC servers
retry_secret_path = "/etc/expressgateway/quic/passthrough-retry.secret"
# max_quic_connections = 100000   # default; the flow-table budget
# flow_idle_timeout_ms = 60000    # default; idle-flow reaper window
# mint_retry           = true     # default; keep ON (see below)
```

Why it's shaped this way:

- **`backends`** are QUIC servers — Mode A does not terminate, so the
  upstream must speak QUIC directly. Connection-ID hashing spreads flows
  across them and pins each flow to one backend for its lifetime.
- **`mint_retry = true`** (default) makes the gateway issue a stateless
  QUIC Retry token bound to the peer address before admitting a flow. This
  is the Initial-flood / source-spoofing defense — keep it on. The
  trade-off (the table cap is a global budget with no per-IP sub-cap) is
  documented in [`known-limitations.md`](../known-limitations.md) "Mode A
  passthrough relies on the QUIC Retry round-trip".
- **`max_quic_connections`** sizes the flow table (the routing table is
  twice this). The other `[passthrough]` knobs and their foot-gun disable
  values are in [`CONFIG.md`](CONFIG.md) "`[passthrough]`".

**Verify.** With an HTTP/3-capable client pointed at the VIP, a request
should complete end-to-end (the backend, not the gateway, terminates TLS):

```bash
curl -vk --http3 https://<vip>:443/ 2>&1 | grep -i 'HTTP/3'
# < HTTP/3 200
```

Because the gateway never decrypts, you confirm correctness at the
**backend**: the backend's TLS logs show the real client SNI/handshake (not
the gateway's), proving traffic passed through opaque.

---

## Combined Configuration Example

A production config for an HTTPS edge that serves HTTP/1.1 + HTTP/2,
advertises and serves HTTP/3, is DoS-hardened, and exposes metrics. This is
the copy-and-adapt artifact; every block links to its recipe and to
[`CONFIG.md`](CONFIG.md).

```toml
# ── Process-wide knobs ──────────────────────────────────────────────
[runtime]
per_ip_connection_cap    = 512        # Recipe 4: per-source-IP connection cap
max_inflight_connections = 65536      # Recipe 4: per-listener admission ceiling

[runtime.tls]
tls13_only = false                    # allow TLS 1.2 (downgrade-safe) + 1.3

[runtime.watchdog]
header_deadline_ms    = 5000          # Recipe 4: slowloris floor
body_progress_min_bps = 64            # Recipe 4: slow-POST floor

[security]
strict_te = true                      # Recipe 4: reject non-chunked Transfer-Encoding

# ── Metrics / health admin listener (loopback) ──────────────────────
[observability]
metrics_bind = "127.0.0.1:9090"       # see observability.md

# ── HTTPS front (TCP): HTTP/1.1 + HTTP/2, advertises HTTP/3 ──────────
[[listeners]]
address          = "0.0.0.0:443"
protocol         = "h1s"              # Recipe 1
drain_timeout_ms = 30000             # generous drain for keep-alive clients

[listeners.tls]
cert_path = "/etc/expressgateway/tls/cert.pem"
key_path  = "/etc/expressgateway/tls/key.pem"

[listeners.alt_svc]
h3_port = 443                         # Recipe 3: advertise H3

[[listeners.backends]]
address  = "10.0.0.10:8080"
protocol = "h1"

[[listeners.backends]]
address  = "10.0.0.11:8080"
protocol = "h1"

# ── HTTP/3 front (QUIC/UDP, same port) ──────────────────────────────
[[listeners]]
address  = "0.0.0.0:443"
protocol = "quic"                     # Recipe 3

[listeners.quic]
cert_path         = "/etc/expressgateway/tls/cert.pem"
key_path          = "/etc/expressgateway/tls/key.pem"
retry_secret_path = "/etc/expressgateway/quic/retry.secret"

[[listeners.backends]]
address  = "10.0.0.10:8080"
protocol = "h1"

[[listeners.backends]]
address  = "10.0.0.11:8080"
protocol = "h1"
```

**Verify the whole thing.**

```bash
# 1) The process booted — a bad config exits non-zero and starts nothing.
target/release/expressgateway /etc/expressgateway/config.toml &

# 2) H2 negotiates and proxies:
curl -vk --http2 https://127.0.0.1:443/ 2>&1 | grep -E 'ALPN|HTTP/2'

# 3) H3 is advertised:
curl -sIk https://127.0.0.1:443/ | grep -i alt-svc

# 4) Metrics are live:
curl -s http://127.0.0.1:9090/metrics | head
```

To apply a backend-pool change to a running instance without dropping
connections, edit the file and `SIGHUP` — the backend pool and the
per-listener HTTP timeouts swap live; everything else is restart-required
and is logged, not silently applied. See [`CONFIG.md`](CONFIG.md) "Reload
semantics".

## See also

- [`CONFIG.md`](CONFIG.md) — the per-knob reference for everything above.
- [`getting-started.md`](getting-started.md) — first run, from zero.
- [`deployment-patterns.md`](deployment-patterns.md) — how to run N of these.
- [`observability.md`](observability.md) — what to watch once it's serving.
- [`troubleshooting.md`](troubleshooting.md) — when a recipe doesn't behave.
- [`glossary.md`](../glossary.md) — ALPN, Connection ID, Mode A/B, and the rest.
- [`config/examples/`](../../config/examples/) — one runnable file per shape.
