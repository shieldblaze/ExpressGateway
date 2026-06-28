# Configuration Reference

ExpressGateway is configured via a single TOML file passed as the first
CLI argument (`expressgateway <config.toml>`); when omitted it defaults to
`config/default.toml`. The binary reads the file, parses it
(`lb_config::parse_config`), validates it (`lb_config::validate_config`),
and **aborts boot with a non-zero exit** if either step fails — a
misconfigured gateway never starts half-up.

This document inventories every key the parser
(`crates/lb-config/src/lib.rs`) accepts. The schema is the authority; this
file tracks it. Runnable examples per listener type live in
`config/examples/`.

## Strict parsing (S37-B)

Every config struct carries `#[serde(deny_unknown_fields)]`. An unknown or
misspelled key — a typo'd block (`[listenrs]`), a fat-fingered knob
(`max_keepalv_requests = 5`) — is a **parse error**, not a silent drop.
Before S37 there was no `deny_unknown_fields` anywhere, so unknown keys
parsed clean and the operator's override was silently ignored. If the
gateway rejects a key you expect, check the spelling and the block it
belongs under.

### Validating a config (there is no dry-run flag)

There is **no dedicated `--check` / dry-run flag** today. The binary validates
the *whole* config at boot — parse (`deny_unknown_fields`) then
`validate_config` — and **exits non-zero without starting any listener** on the
first parse or validation error (fail-fast; see "Invalid-config behavior"
below). So a CI "does this config parse?" check means **booting the binary
against it**, which has side effects: it binds the configured listener ports
and, for a `quic` / `[passthrough]` config, mints the QUIC retry-secret file
(mode 0600) if it is absent. Run such a check against a throwaway config (free
ports, a temp `retry_secret_path`) or in a sandbox, and stop the process once
it logs its listener lines. A validate-only mode that parses and validates
without binding is a deferred enhancement.

## Foot-gun knobs (`0` / `false` = disable a defense)

A handful of knobs accept a sentinel value that **turns off a protection**.
These are documented, range-bounded escapes (not silent), but they weaken
the gateway — set them only for trusted listeners and understand the
trade-off. The S38 security audit enumerated and confirmed each one is a
documented sentinel, not a silent bypass (`audit/security/s38-findings-infra.md`
L-INFRA-1).

| Knob | Disable value | What turning it off costs |
|------|---------------|---------------------------|
| `[runtime].max_keepalive_requests` | `0` | No proactive H1/H2 keep-alive connection recycling. |
| `[runtime].max_requests_per_h3_connection` | `0` | **Re-opens the quiche `StreamMap::collected` leak / H3 stream-flood DoS** that S36 closed. Trusted listeners only. |
| `[runtime].xdp_new_flow_cap_per_sec_per_cpu` | `0` | No per-CPU new-flow (SYN-flood) rate cap in XDP. |
| `[[listeners.backends]].tls_verify_peer` | `false` | **Disables H3-backend certificate verification — accepts a MITM upstream.** Valid only on `h3` backends; rejected elsewhere. |
| `[passthrough].mint_retry` | `false` | No stateless Retry on Mode A → re-opens the QUIC Initial address-spoof flood vector. |
| `[passthrough].strict_source_binding` | `false` (default) | Permits NAT-rebind connection migration (the default); `true` hardens against off-path 4-tuple confusion at the cost of breaking legitimate rebinds. |
| `[passthrough].flow_idle_timeout_ms` | `0` | No idle-flow reaper (LRU eviction only). |

All other range-bounded knobs reject out-of-range values at parse time
(e.g. `max_keepalive_requests > 10_000_000` is a hard error), so a
fat-finger that would *silently* break a defense is caught.

## Minimum working config

`config/default.toml` is a minimal accepted config:

```toml
[[listeners]]
address = "0.0.0.0:8080"
protocol = "tcp"

[[listeners.backends]]
address = "127.0.0.1:3000"
weight = 1

[[listeners.backends]]
address = "127.0.0.1:3001"
weight = 1
```

This produces a single plain-TCP listener on port 8080 that load-balances
round-robin across the two backends. (`weight` is shown here for
completeness but is **not yet enforced** — the live picker is round-robin
regardless of weight; see the `[[listeners.backends]]` table below.) At
least one `[[listeners]]` **or** a top-level `[passthrough]` block is
required.

## Worked examples (`config/examples/`)

Each example boots with `expressgateway config/examples/<file>`. Pick the
one that matches your deployment:

| Example | Deployment it models |
|---------|----------------------|
| `tcp.toml` | Plain L4 TCP load balancing — raw byte shovel across a round-robin backend pool. Use for non-HTTP protocols or pure L4. |
| `tls.toml` | L4 TLS termination — rustls decrypts the client TLS and forwards the cleartext byte stream to a raw-TCP backend (no HTTP awareness). |
| `h1.toml` | L7 HTTP/1.1 over plain TCP — hyper-terminated, with the security hooks (header policy, smuggle defense, timeouts) and per-listener HTTP timeouts. |
| `h1s.toml` | L7 HTTPS — HTTP/1.1 **and HTTP/2** over TLS on one port via ALPN (`h2` preferred, `http/1.1` fallback). This is the **only way HTTP/2 is served**. |
| `h1s-grpc.toml` | gRPC over HTTP/2 (TLS) — adds `[listeners.grpc]` (deadline clamp + synthesized health check); the backend speaks `h2`. gRPC needs an H2 or H3 front. |
| `h1s-websocket.toml` | WebSocket over HTTPS — RFC 6455 WS-over-HTTP/1.1, on by default once `[listeners.websocket]` is present. WS-over-HTTP/2 (RFC 8441) is opt-in and **off by default** (CF-S27-2). |
| `quic-h3.toml` | HTTP/3 — a `quic` listener in H3-terminate mode (the default QUIC mode). This is the **only way HTTP/3 is served**; advertise it from an h1/h1s listener via `[listeners.alt_svc]`. |
| `quic-mode-b.toml` | Mode B raw-QUIC proxy — `[listeners.quic.raw_proxy]` terminates the client QUIC and re-originates a dedicated upstream QUIC connection, relaying raw streams + datagrams (two distinct quiche connections). |
| `passthrough-mode-a.toml` | Mode A QUIC passthrough — a top-level `[passthrough]` block routes QUIC by Connection ID **without decrypting** (TLS stays end-to-end client↔backend). A parallel datapath; valid as the *only* datapath. |

`config/default.toml` is the minimal accepted config (a single plain-TCP
listener); it is what the binary loads when you pass no path.

## Listener protocols (what the binary actually serves)

The binary's listener spawn loop (`crates/lb/src/main.rs`) branches
`protocol == "quic"` → `spawn_quic` (UDP) versus everything-else →
`spawn_tcp` (TCP), and the TCP path dispatches a running implementation
for exactly `tcp` / `tls` / `h1` / `h1s`. The **served listener-protocol
set** is therefore:

| `protocol` | Transport | Notes |
|-----------|-----------|-------|
| `"tcp"`  | plain TCP (L4) | Raw byte shovel to the backend. |
| `"tls"`  | TLS over TCP | rustls termination. Requires `[listeners.tls]`. |
| `"h1"`   | HTTP/1.1 over TCP | hyper-terminated L7 proxy. |
| `"h1s"`  | HTTP/1.1 + HTTP/2 over TLS | ALPN advertises `h2` (preferred) then `http/1.1`. **HTTP/2 is served here** — there is no separate `"h2"` listener. Requires `[listeners.tls]`. |
| `"quic"` | QUIC over UDP | Two modes: H3-terminate (default) or Mode B raw-QUIC proxy (`[listeners.quic.raw_proxy]`). **HTTP/3 is served here.** |

**S37-B:** `"http"`, `"h2"`, and `"h3"` are **rejected as listener
protocols** by `validate_config`. They are never served — before S37 they
validated clean and then hard-errored at boot with "no runtime
implementation". Use `"h1s"` (ALPN) for HTTP/2 and `"quic"` for HTTP/3.
(These same tokens *are* valid for `[[listeners.backends]].protocol` —
that is the upstream wire protocol, a different axis.)

## `[[listeners]]` — array of listener blocks

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `address` | `String` (socketaddr or `host:port`) | required | Bind address. IPv6 literals must be bracketed: `[::1]:8080`. Must be non-empty. |
| `protocol` | `String` | required | One of the served set above. |
| `tls` | `[listeners.tls]` | absent | Required when `protocol = "tls"` or `"h1s"`; forbidden otherwise. |
| `quic` | `[listeners.quic]` | absent | Required when `protocol = "quic"`; forbidden otherwise. |
| `alt_svc` | `[listeners.alt_svc]` | absent | `Alt-Svc: h3` advertisement. Meaningful for `h1`/`h1s`. |
| `http` | `[listeners.http]` | absent | Per-listener HTTP timeouts. Meaningful for `h1`/`h1s`. |
| `h2_security` | `[listeners.h2_security]` | absent | HTTP/2 security thresholds. Meaningful for `h1s` (H2 via ALPN). |
| `websocket` | `[listeners.websocket]` | absent | WebSocket capability. Meaningful for `h1`/`h1s`/`quic`. |
| `grpc` | `[listeners.grpc]` | absent | gRPC capability. Requires `protocol = "h1s"`. |
| `drain_timeout_ms` | `Option<u64>` (`100..=300000`) | inherit `[runtime].drain_timeout_ms` | OPS-10: per-listener graceful-drain budget. |
| `drain_jitter_ms` | `Option<u64>` (`0..=` effective `drain_timeout_ms`) | inherit derived `[runtime]` jitter | OPS-02: per-listener drain-cancel jitter ceiling. `0` disables jitter. |
| `backends` | `[[listeners.backends]]` array | `[]` | Backend pool. A non-QUIC listener with an empty pool logs a warning and is skipped. For a `quic` H3-terminate listener the binary forwards to the **first** backend in the pool (H3-terminate is not load-balanced across multiple backends — see [Load balancing](../features.md#load-balancing)). |

### `[[listeners.backends]]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `address` | `String` (socketaddr or `host:port`) | required | Resolved via `lb_io::DnsResolver` at startup. Must be non-empty. |
| `protocol` | `String` (`tcp`/`h1`/`h2`/`h3`) | `"tcp"` | Upstream wire protocol. `tcp`/`h1` → HTTP/1.1 (or raw); `h2` → HTTP/2 over TCP+TLS (ALPN); `h3` → HTTP/3 over QUIC. |
| `weight` | `u32` | `1` | **Accepted but not yet enforced** — the live picker is round-robin and ignores `weight` in this build (weighted selection is implemented in the `lb-balancer` library but not wired). See [Load balancing](../features.md#load-balancing). |
| `tls_ca_path` | `Option<String>` | absent | PEM CA bundle to verify an `h3` backend. Required for `h3` unless `tls_verify_peer = false`. Rejected on non-`h3` backends. |
| `tls_verify_hostname` | `Option<String>` | host of `address` | SNI override for `h3` backend verification. Rejected on non-`h3` backends. |
| `tls_verify_peer` | `bool` | `true` | Set `false` to disable `h3` backend cert verification (**NOT RECOMMENDED**). Rejected on non-`h3` backends. |

A `quic` H3-terminate listener forwards to **exactly one** backend
protocol family (h1/tcp, h2, or h3) — mixing families is rejected, as is
combining `[listeners.quic.raw_proxy]` with `[[listeners.backends]]`.

### `[listeners.tls]` (required for `tls` / `h1s`)

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `cert_path` | `String` | required | PEM certificate chain. |
| `key_path` | `String` | required | PEM private key (PKCS#8 or SEC1). |
| `ticket_rotation_interval_seconds` | `u64` (`> 0`) | `86400` | Session-ticket key rotation window. |
| `ticket_rotation_overlap_seconds` | `u64` | `86400` | Grace window for tickets under the previous key. |

### `[listeners.quic]` (required for `quic`)

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `cert_path` | `String` | required | PEM certificate chain. |
| `key_path` | `String` | required | PEM private key. |
| `retry_secret_path` | `String` | required | 32-byte retry-token signing key; minted with mode 0600 on first boot if absent. |
| `max_idle_timeout_ms` | `u64` (`> 0`) | `30000` | QUIC connection idle timeout. |
| `max_recv_udp_payload_size` | `u64` (`>= 1200`) | `1350` | Max accepted UDP payload (RFC 9000 §14). |
| `raw_proxy` | `[listeners.quic.raw_proxy]` | absent | Present ⇒ Mode B raw-QUIC proxy; absent ⇒ H3-terminate. |

### `[listeners.quic.raw_proxy]` (Mode B)

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `backend_addr` | `String` (`host:port`) | required | Upstream QUIC backend to re-originate to. |
| `sni` | `String` | required | SNI for the re-originated TLS handshake. |
| `backend_ca_path` | `Option<String>` | absent | PEM CA to verify the backend. Absent ⇒ system trust roots (verification stays ON). |
| `dgram_queue_cap` | `usize` | `1024` | Per-direction bounded DATAGRAM relay queue. |
| `max_relay_streams` | `usize` | `256` | Per-connection relay stream-table ceiling. |

### `[listeners.alt_svc]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `h3_port` | `u16` | required | UDP port of the H3 listener to advertise via `Alt-Svc`. |
| `max_age` | `u32` | `3600` | `ma=` value (seconds). |

### `[listeners.http]` (timeouts; all `> 0`)

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `header_timeout_ms` | `u64` | `10000` | Request-line + headers read budget. |
| `body_timeout_ms` | `u64` | `30000` | Body read / upstream-body budget. |
| `total_timeout_ms` | `u64` | `60000` | Hard total-request-lifetime backstop. |
| `head_timeout_ms` | `u64` | `60000` | Post-upload head-wait cap (CF-BODY-WALLCLOCK). |

### `[listeners.h2_security]`

All fields are `Option` and default to the live `H2SecurityThresholds`
defaults when omitted: `max_pending_accept_reset_streams`,
`max_local_error_reset_streams`, `max_concurrent_streams`,
`max_header_list_size`, `max_send_buf_size`, `keep_alive_interval_ms`,
`keep_alive_timeout_ms`, `initial_stream_window_size`,
`initial_connection_window_size`. See `audit/decisions/h2-edge-streams.md`.

### `[listeners.websocket]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | `bool` | `true` (when the block is present) | Master switch. |
| `idle_timeout_seconds` | `u64` (`> 0`) | `60` | Both-directions-idle close (1001). |
| `max_message_size_bytes` | `usize` (`> 0`) | `16777216` | Single-message cap. |
| `ping_rate_limit_per_window` | `u32` (`> 0`) | `50` | Client-Ping flood cap (then Close 1008). |
| `ping_rate_limit_window_seconds` | `u64` (`> 0`) | `10` | Ping rate window. |
| `read_frame_timeout_seconds` | `u64` (`> 0`) | `30` | Per-direction read-frame watchdog. |
| `h2_extended_connect` | `bool` | `false` | RFC 8441 WS-over-H2. OFF by default (CF-S27-2 backpressure). |
| `h3_extended_connect` | `bool` | `false` | RFC 9220 WS-over-H3. OFF by default. |

### `[listeners.grpc]` (requires `protocol = "h1s"`)

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | `bool` | `true` | Master switch. |
| `max_deadline_seconds` | `u64` (`> 0`) | `300` | Upper bound on accepted `grpc-timeout`. |
| `health_synthesized` | `bool` | `true` | Serve `/grpc.health.v1.Health/Check` locally. |

## `[runtime]` — process-wide knobs (optional)

When absent, every default below applies. (Contrary to older docs, this
block **is** parsed and acted on.)

| Key | Type | Range | Default | Description |
|-----|------|-------|---------|-------------|
| `xdp_enabled` | `bool` | — | `false` | Attach the XDP data-plane on boot. Requires `xdp_interface`. |
| `xdp_interface` | `Option<String>` | non-empty when `xdp_enabled` | absent | NIC to attach XDP to. |
| `xdp_mode` | `"auto"\|"native"\|"skb"\|"hw"` | — | `"auto"` | XDP attach-mode selector. |
| `drain_timeout_ms` | `u64` | `100..=300000` | `10000` | Graceful-drain budget on SIGTERM. |
| `readiness_settle_ms` | `u64` | `0..=30000` | `11000` | Settle window between `/readyz` 503 and cancel. |
| `drain_jitter_ms` | `Option<u64>` | `0..=drain_timeout_ms` | derive `drain_timeout_ms / 4` | Drain-cancel jitter ceiling. `Some(0)` disables. |
| `handshake_timeout_ms` | `u64` | `100..=60000` | `5000` | TLS handshake wall-clock cap. |
| `max_inflight_connections` | `u32` | `100..=2000000` | `65536` | Per-listener inflight cap (semaphore). |
| `connect_timeout_ms` | `u64` | `100..=60000` | `5000` | Upstream dial timeout. |
| `per_ip_connection_cap` | `u32` | `1..=2000000` | `1024` | Per-source-IP concurrent-connection cap. |
| `header_underscore_policy` | `"reject"\|"drop"\|"allow"` | — | `"reject"` | Handling of `_` in inbound header names. **Only the default `reject` is enforced in this build**; `drop`/`allow` parse and validate but are not yet applied by the proxies (the proxy keeps its compile-time `reject` default). |
| `max_keepalive_requests` | `u32` | `0` (disable) or `1..=10000000` | `100` | H1/H2 requests per keep-alive connection before proactive close. **S37-B** added the range check (was unvalidated). |
| `max_requests_per_h3_connection` | `u32` | `0` (disable) or `1..=10000000` | `1000` | H3 requests per connection before GOAWAY + recycle (S36-A; bounds quiche `StreamMap::collected`). `0` re-opens the leak/DoS vector — trusted listeners only. **S37-B** added the range check. |
| `xdp_new_flow_cap_per_sec_per_cpu` | `u32` | `0` (disable) or `1000..=10000000` | `125000` | Per-CPU new-flow rate cap (Katran parity). |
| `tls` | `[runtime.tls]` | — | absent | Process-wide TLS policy. |
| `watchdog` | `[runtime.watchdog]` | — | absent | Slowloris / slow-POST watchdog. |

### `[runtime.tls]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `tls13_only` | `bool` | `false` | TLS 1.3 only on every TLS listener. |

### `[runtime.watchdog]`

| Key | Type | Range | Default | Description |
|-----|------|-------|---------|-------------|
| `header_deadline_ms` | `u64` | `100..=60000` | `5000` | Per-request header-phase deadline. |
| `body_progress_min_bps` | `u64` | `0..=10000000` | `64` | Slow-POST rate floor (B/s); `0` disables. |
| `sweep_interval_ms` | `u64` | `100..=60000` | `1000` | Stalled-connection sweep cadence. |

## `[observability]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `metrics_bind` | `Option<String>` (socketaddr) | absent | Admin HTTP listener for `/metrics`, `/livez`, `/readyz`. When absent, no admin listener binds. Non-loopback binds require `[admin]`. Log level is via `RUST_LOG` (see `RUNBOOK.md`), not TOML. |

## `[admin]` (SEC-2-06)

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `api_token_hash` | `Option<String>` | absent | 64-char hex SHA-256 of the admin bearer token. |
| `allow_non_loopback` | `bool` | `false` | Allow the admin listener to bind non-loopback. Requires `api_token_hash` when `true`. |

## `[security]` (PROTO-2-17)

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `strict_te` | `bool` | `false` | `true` ⇒ `SmuggleMode::H1Strict` (reject any `Transfer-Encoding` codec other than `chunked`). |

## `[passthrough]` (Mode A QUIC passthrough)

A top-level block (not under `[[listeners]]`) running the stateless
QUIC-by-Connection-ID passthrough datapath. May be the **only** datapath
(no `[[listeners]]` required).

| Key | Type | Range | Default | Description |
|-----|------|-------|---------|-------------|
| `bind_addr` | `SocketAddr` | — | required | Listener UDP socket. |
| `backends` | `[SocketAddr]` | non-empty | required | Maglev-hashed backend pool. |
| `retry_secret_path` | `PathBuf` | non-empty | required | 32-byte retry secret; minted 0600 if absent. |
| `max_quic_connections` | `usize` | `1..=2000000` | `100000` | Flow cap (routing table = `2 * cap`). |
| `min_client_dcid_len` | `usize` | `8..=20` | `8` | Min client DCID (cross-flow prefix-collision defence). |
| `per_flow_backlog` | `usize` | `1..=8192` | `32` | Per-flow datagram backlog (drop-newest). |
| `strict_source_binding` | `bool` | — | `false` | Drop short-header packets whose 4-tuple differs from the flow's peer. |
| `audit_throttle_window_secs` | `u64` | `> 0` | `60` | Audit-log throttle window. |
| `max_dcid_len_routed` | `usize` | `1..=20` | `20` | Short-header DCID length to try first. |
| `mint_retry` | `bool` | — | `true` | Mint stateless Retry on no-token Initials. |
| `flow_idle_timeout_ms` | `u64` | — | `60000` | Idle-flow reaper window; `0` disables (LRU-only). |

## Reload semantics (SIGHUP hot reload)

The binary supports **validate-first SIGHUP config hot reload** (S37-C).
`systemctl reload expressgateway` (or `kill -HUP <pid>`) re-reads the
config file on disk, parses + validates it, and — only if it is valid —
applies the **swappable** subset live without dropping in-flight
connections. The reload follows an **honesty contract**: every field is
classified as either *swappable* or *restart-required*, and a
restart-required change is **never silently applied**.

| Reload class | Fields | Behaviour on SIGHUP |
|---|---|---|
| **Swappable** (applied live) | `[[listeners.backends]]` pool, the per-listener `[listeners.http]` timeouts | L7 proxy rebuilt + atomically swapped behind an `ArcSwap`; existing connections finish on their captured snapshot (RCU), new connections see the new config. |
| **Restart-required** (logged, NOT applied) | `protocol`, `address`, `[listeners.tls]`, `[listeners.quic]`, `max_requests_per_h3_connection`, all `drain_*`, `[admin]`, `[security]`, `[passthrough]`, all `[runtime]`/XDP fields | Diff is logged (`field=… SIGHUP: …`) and `config_reload_restart_required_fields_total` is bumped; the live config keeps serving until you restart the process. |

If the new config fails to parse or validate, the reload is **rejected
atomically**: nothing is applied, the live config keeps running, and the
gateway logs `SIGHUP: … rolling back, keeping live config` and bumps
`config_reload_failed_total`. A successful swappable apply bumps
`config_reload_succeeded_total` + `config_reload_applied_swappable_total`.
See `RUNBOOK.md` "Configuration reload".

**TLS certificate rotation is a separate path** on **SIGUSR1**: it
re-reads the cert/key files for TLS/`h1s` listeners and atomically swaps
the `TlsConfigBundle` (bumping `cert_rotation_*` metrics), keeping the
previous session-ticket key valid for its overlap window. Use SIGUSR1
after writing new cert/key PEMs; use SIGHUP after editing the TOML.

## Validation

`lb_config::validate_config` runs after parse and rejects, among others:

- a config with neither `[[listeners]]` nor `[passthrough]`;
- empty listener `address` / `protocol`;
- an unserved listener protocol (`http`/`h2`/`h3`) — see S37-B above;
- an unknown listener protocol;
- a `[listeners.tls]` / `[listeners.quic]` block on the wrong protocol, or
  a missing one when required;
- numeric knobs outside their documented ranges (see the tables);
- an `h3` backend without `tls_ca_path` (unless `tls_verify_peer = false`),
  or `tls_*` knobs on a non-`h3` backend;
- a `quic` listener mixing backend protocol families, or combining
  `raw_proxy` with `[[listeners.backends]]`;
- a `[listeners.grpc]` block on a non-`h1s` listener.

Plus, from `deny_unknown_fields` at parse time: any unknown/misspelled key
in any block.

## Invalid-config behavior

On a parse error or a validation error, the binary exits non-zero and
prints the `anyhow`-wrapped error chain to stderr — it does **not** start a
partial listener set. Real-binary boot proofs (positive boots per protocol
+ refusal on an unserved protocol / unknown key) live in
`tests/config_boot_matrix.rs`; the per-error-class negative coverage of
`parse_config` / `validate_config` lives in the `crates/lb-config` unit
tests.
