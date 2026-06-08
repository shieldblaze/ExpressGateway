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
round-robin across two backends. At least one `[[listeners]]` **or** a
top-level `[passthrough]` block is required.

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
| `backends` | `[[listeners.backends]]` array | `[]` | Backend pool. A non-QUIC listener with an empty pool logs a warning and is skipped. For a `quic` H3-terminate listener the binary forwards to backends only through the library/e2e path (see `config/examples`). |

### `[[listeners.backends]]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `address` | `String` (socketaddr or `host:port`) | required | Resolved via `lb_io::DnsResolver` at startup. Must be non-empty. |
| `protocol` | `String` (`tcp`/`h1`/`h2`/`h3`) | `"tcp"` | Upstream wire protocol. `tcp`/`h1` → HTTP/1.1 (or raw); `h2` → HTTP/2 over TCP+TLS (ALPN); `h3` → HTTP/3 over QUIC. |
| `weight` | `u32` | `1` | Relative weight for round-robin selection. |
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
| `header_underscore_policy` | `"reject"\|"drop"\|"allow"` | — | `"reject"` | `_` in inbound header names. |
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

## Reload semantics

As of this revision the binary loads the config **once at boot**; there is
no live reload of `[[listeners]]` / backends. TLS certificate hot-reload is
the only runtime config change today (SIGUSR1 re-reads cert/key files for
TLS/`h1s` listeners and bumps `cert_rotation_*` metrics) — the rest of the
config is fixed for the process lifetime. (A validate-first / atomic-swap
SIGHUP reload is a separate in-progress workstream; this section will be
updated when it lands.)

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
