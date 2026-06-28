# Troubleshooting

First aid for the common ways a deployment goes wrong, indexed by
**symptom**: what you see → the likely cause → the fix → how to confirm.

This page covers **setup and first-run** failures — config rejected, won't
boot, 502s, a protocol that won't negotiate. For a **live, already-serving**
instance throwing alerts (5xx rate, slow drains, accept saturation, XDP
attach mode, cert rotation), the [`RUNBOOK.md`](RUNBOOK.md) alert catalog is
the canonical triage — each alert there has a diagnose procedure and a
first-command matrix. This page does not duplicate it; it links to it.

## How to debug it

Before reaching for a specific symptom below, two habits save the most time:

1. **Read the actual error.** A misconfigured gateway never starts half-up
   — on any parse or validation failure it prints the full error chain to
   stderr and **exits non-zero, binding nothing**. The error chain usually
   names the offending key or file. Run it in the foreground and read the
   last few lines before assuming anything.
2. **Trust the logs' order.** A healthy boot logs a fixed sequence
   (runtime ready → backend pool ready → DNS resolver ready → one line per
   listener). The *first missing* line tells you which stage failed. The
   expected startup sequence is in [`RUNBOOK.md`](RUNBOOK.md) "Startup".

## First-run FAQ

**How do I pass the config file?** As a **positional argument** — there is
no `--config` flag:

```bash
target/release/expressgateway /etc/expressgateway/config.toml   # correct
target/release/expressgateway --config /etc/.../config.toml     # WRONG
```

The second form makes the binary try to open a file literally named
`--config` and fail with `cannot read config file: --config`. With no
argument at all it loads `config/default.toml`.

**Which listener serves HTTP/2? HTTP/3?** HTTP/2 is served by an **`h1s`**
listener (ALPN negotiates `h2`); HTTP/3 by a **`quic`** listener. There is
no `h2` or `h3` *listener* protocol — those tokens are rejected at boot.
(They *are* valid as a backend `protocol`, which is a different axis.) See
[`CONFIG.md`](CONFIG.md) "Listener protocols".

**`/metrics` and the health probes return nothing.** The admin listener is
off until you bind it: add `[observability] metrics_bind = "127.0.0.1:9090"`.
See [`observability.md`](observability.md).

**Is there compression / response caching / a WAF?** No — ExpressGateway is
a streaming pass-through proxy and does not transform bodies. See
[`comparison.md`](comparison.md).

## Symptom: config rejected — "unknown field" / "unknown variant"

**Cause.** Parsing is strict: every config block rejects keys it doesn't
recognise. A typo'd key (`max_keepalv_requests`), a mis-spelled block
(`[listenrs]`), or a key placed under the wrong block is a **hard parse
error**, not a silent drop. This is deliberate — it stops a fat-fingered
override from being silently ignored.

**Fix.** Read the error: it names the unknown field and the block it was
found in. Check the spelling and that the key belongs under that block in
[`CONFIG.md`](CONFIG.md). For value-range errors (e.g. a number outside its
allowed range), the same reference lists every range.

**Confirm.** Re-run in the foreground; a clean boot prints the listener
lines and stays up.

## Symptom: won't boot / exits non-zero

**Causes and fixes** (the error chain names which one):

- **A TLS/QUIC `cert_path` or `key_path` is wrong**, or the **private key is
  group/other-readable** — keys must be mode `0600` or they're rejected.
  Fix the path/permissions.
- **A `[listeners.tls]` / `[listeners.quic]` block is missing or on the
  wrong protocol** — `tls`/`h1s` require `[listeners.tls]`; `quic` requires
  `[listeners.quic]`; both are forbidden on a plain `tcp`/`h1` listener.
- **An unserved listener `protocol`** (`http`, `h2`, `h3`) — use `h1s` for
  HTTP/2 and `quic` for HTTP/3 (see the FAQ above).
- **A `quic` listener mixing backend protocol families**, or combining
  `[listeners.quic.raw_proxy]` (Mode B) with `[[listeners.backends]]` — an
  H3-terminate listener forwards to one family (h1/tcp, h2, *or* h3).
- **A non-loopback admin bind without the override and a token** — binding
  `[observability].metrics_bind` to a non-loopback address requires
  `[admin].allow_non_loopback = true` **and** `[admin].api_token_hash`.
- **The label-cardinality budget would be exceeded** — the boot-time check
  refuses to start if the config shape would emit more metric series than
  the ceiling. The error names the family and the count; see
  [`METRICS.md`](METRICS.md).
- **Neither `[[listeners]]` nor `[passthrough]` is present** — at least one
  datapath is required.

**Confirm.** Fix the named issue and re-run; the binary either boots clean
or prints the next error.

## Symptom: every request returns 502

A 502 means the gateway accepted the client request but could not get a
usable response from the backend. The usual causes:

- **Backend unreachable** — it's down, the address/port is wrong, or a
  firewall/security-group blocks the gateway→backend path. The dial is
  bounded by `[runtime].connect_timeout_ms`.
- **DNS resolution failed** — hostname backends are resolved at
  startup/reload; a bad name or resolver fails the pool. Check the
  `DNS resolver` log lines.
- **Backend protocol mismatch** — the backend `protocol` must match what
  the upstream actually speaks. Pointing a `protocol = "h2"` backend at an
  HTTP/1.1 server (or vice versa) fails every request.

**Fix.** Verify the backend address is reachable from the gateway host
(`curl`/`nc` to it directly), that hostnames resolve, and that the backend
`protocol` matches the upstream. Re-check after `SIGHUP` if you edited the
pool.

**Confirm.** Attribute it with `backend_requests_total{status_class="5xx"}`
to find which upstream is failing; for the live-traffic version of this,
[`RUNBOOK.md`](RUNBOOK.md) `LbReq5xx` has the full triage.

## Symptom: gRPC client gets no status / RST_STREAM

**Cause.** The client is behind an **HTTP/1.1 front**. gRPC carries its
status in HTTP trailers, and an HTTP/1.1 downstream cannot deliver trailers
on a streamed response — so `grpc-status` never arrives. This matches nginx
and is a property of HTTP/1.1, not a defect.

**Fix.** Terminate gRPC clients on an **`h1s` (HTTP/2)** or **`quic`
(HTTP/3)** listener, with an `h2`/`h3` backend. See
[Recipe 2](cookbook.md#recipe-2--terminating-grpc) and the full rationale
in [`known-limitations.md`](../known-limitations.md) "gRPC requires an
HTTP/2 or HTTP/3 front".

**Confirm.** `grpcurl` against an H2 front returns a real status; the
synthesized health check answers `SERVING`.

## Symptom: WebSocket fails over HTTP/2

**Cause.** WebSocket-over-HTTP/2 (RFC 8441 extended CONNECT) is **gated off
by default**. When it's off, an H2 extended-CONNECT request is rejected
identically to the feature being absent. (WebSocket over HTTP/1.1 and over
HTTP/3 are unaffected.)

**Fix.** Run WebSocket over **H1** (the default) or **H3**. Only enable
`websocket.h2_extended_connect = true` behind a trusted, well-behaved client
tier — the gate exists because the H2 tunnel can buffer unbounded against a
stalled peer. Background in [`known-limitations.md`](../known-limitations.md)
"WebSocket over HTTP/2 is gated OFF by default".

**Confirm.** The same client over an H1 WebSocket listener (see
[`config/examples/h1s-websocket.toml`](../../config/examples/h1s-websocket.toml))
upgrades and echoes.

## Symptom: cert reload didn't take

**Cause.** The wrong signal. **Certificate/key files reload on `SIGUSR1`.**
`SIGHUP` reloads the *config TOML*, and on `SIGHUP` the `[listeners.tls]`
block is treated as restart-required (logged, not applied) — so sending
`SIGHUP` after writing new PEMs does nothing to the live cert.

**Fix.** Write the new PEMs atomically (rename, don't truncate; key `0600`),
then send **`SIGUSR1`** (`kill -USR1 <pid>`). The bundle swaps atomically;
on a parse/validate failure the old cert stays live. Full procedure in
[`DEPLOYMENT.md`](DEPLOYMENT.md) "TLS material" and
[`RUNBOOK.md`](RUNBOOK.md) "TLS certificate rotation".

**Confirm.** `cert_rotation_succeeded_total` increments and the logs show a
rotation line; `cert_loaded_at_seconds` advances.

## Symptom: XDP attach failed, or fell back to `skb`

The XDP/eBPF data plane is **off by default**; this only applies if you set
`[runtime].xdp_enabled = true`. Common causes:

- **Kernel too old** — the effective floor is Linux ≥ 5.15.
- **Missing capabilities** — needs `CAP_BPF` + `CAP_NET_ADMIN` (or
  `CAP_SYS_ADMIN` + `CAP_NET_ADMIN` on pre-5.8 kernels).
- **AWS ENA native-attach constraints** — native (`drv`) attach is refused
  unless `MTU ≤ 3498` and combined channels ≤ half the maximum; otherwise
  the loader demotes to generic (`skb`) with a performance penalty.
- **A known-bad NIC driver/firmware** — the loader refuses native mode on a
  static blocklist to avoid silent packet drops, and demotes to `skb`.

**Fix and confirm.** These are owned end-to-end by the reference docs:
the host requirements and the ENA MTU/channel fix are in
[`DEPLOYMENT.md`](DEPLOYMENT.md) "ENA native-XDP requirements"; the loader
error catalog, the `bpftool`/`ip link` inspection commands, the NIC
blocklist, and the `skb`-fallback alert (`LbXdpAttachMode`) are in
[`RUNBOOK.md`](RUNBOOK.md) "XDP diagnosis". To force generic mode
regardless of NIC, set `[runtime].xdp_mode = "skb"`. With XDP disabled, L4
traffic uses the kernel TCP/UDP stack normally with no loss.

## Live-traffic alerts

Once the instance is serving, ongoing operational signals — elevated 5xx,
slow or truncated drains, accept saturation, pool probe failures, DNS cache
thrash, cert-rotation failure, XDP conntrack full — each have a named alert
with PromQL, severity, and a diagnose procedure in
[`RUNBOOK.md`](RUNBOOK.md) "Alert catalog". Start watching them with the
[`observability.md`](observability.md) golden-signal set.

## See also

- [`getting-started.md`](getting-started.md) — the working first-run path.
- [`CONFIG.md`](CONFIG.md) — every key, range, and the foot-gun table.
- [`cookbook.md`](cookbook.md) — complete configs for each deployment shape.
- [`RUNBOOK.md`](RUNBOOK.md) — live-instance alerts and triage (canonical).
- [`known-limitations.md`](../known-limitations.md) — the bounded behaviors
  behind the gRPC-front, WS-H2, and XDP symptoms above.
- [`glossary.md`](../glossary.md) — unfamiliar term? Look it up here.
