# Configuration Reference

ExpressGateway is configured via a single TOML file passed as the first CLI argument; the default is `config/default.toml`. This document inventories every key the parser (`crates/lb-config/src/lib.rs`) accepts today, the PROMPT.md §19 specification it implements, and the delta.

## Minimum working config

`config/default.toml` is the minimum accepted config:

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

This produces a single L4 TCP listener on port 8080 that load-balances round-robin across two backends.

## Keys (implemented)

### `[[listeners]]` — array of listener blocks

| Key | Type | Default | Reload | Description |
|-----|------|---------|:------:|-------------|
| `address` | `String` (socketaddr or `host:port`) | (required) | restart | Bind address. IPv6 literals must be bracketed: `[::1]:8080`. |
| `protocol` | `String` (`"tcp"`) | `"tcp"` | restart | Listener protocol. Currently only TCP is wired in `crates/lb/src/main.rs`; UDP / QUIC / HTTP are scaffolded but not yet served from the binary. |
| `backends` | array | `[]` | SIGHUP | Backend pool (see below). A listener with an empty pool logs a warning and is skipped. |

### `[[listeners.backends]]`

| Key | Type | Default | Reload | Description |
|-----|------|---------|:------:|-------------|
| `address` | `String` (socketaddr or `host:port`) | (required) | SIGHUP | Backend address. Hostnames (e.g. `origin.example:443`) are resolved through `lb_io::DnsResolver` at startup; IP literals are resolved identically so the code path is uniform. |
| `weight` | `u32` | `1` | SIGHUP | Relative weight for round-robin / weighted-random selection. |

## Keys (specified in PROMPT.md §19, not yet wired)

PROMPT.md §19 describes a broader schema. The parser's `lb_config::ParsedConfig` type exists but the binary only acts on `[[listeners]]` + `[[listeners.backends]]` today. The remaining keys are accepted-but-ignored or return a `ConfigError` on presence.

### Runtime

| Key | Spec type | Status |
|-----|-----------|--------|
| `[runtime]` table | object | Not yet parsed. |
| `runtime.xdp_enabled` | `bool` | Spec, not wired; real XDP requires Pillar 4b (bpf-linker + CAP_BPF). |
| `runtime.io_backend` | `"auto" \| "io_uring" \| "epoll"` | Spec; `Runtime::new()` auto-detects, `Runtime::with_backend` forces, but the TOML key is not routed through the parser yet. |
| `runtime.worker_threads` | `u32` | Currently tokio's default (logical-CPU count). |

### TLS

| Key | Spec type | Status |
|-----|-----------|--------|
| `[[tls.sni_certs]]` | array of { `sni`, `cert_path`, `key_path`, `ocsp_stapling` } | Not wired. `rustls = "0.23"` is in workspace deps (Pillar 3a); binary has no TLS listener yet. |
| `tls.ticket_rotation.interval_seconds` | `u64` | `TicketRotator` exists in `crates/lb-security/src/ticket.rs` (Step 5b); config key is Pillar 3b. |
| `tls.ticket_rotation.overlap_seconds` | `u64` | Same. |
| `tls.mtls.enabled` | `bool` | Not wired. |
| `tls.mtls.ca_path` | `String` | Not wired. |

### HTTP

| Key | Spec type | Status |
|-----|-----------|--------|
| `http.max_header_bytes` | `usize` | Constant `crates/lb-h1/src/parse.rs::MAX_HEADER_BYTES = 65_536` (Step 5a); parameter override via `parse_headers_with_limit` exists but is not surfaced through config. |
| `http.max_body_bytes` | `usize` | Not wired. |
| `http2.max_concurrent_streams` | `u32` | Not wired (no live H2 state machine). |
| `http2.settings_flood_per_window` | `u32` | `SettingsFloodDetector::new(max_per_window, window)` exists; config key is Pillar 3b. |
| `http2.ping_flood_per_window` | `u32` | Same, via `PingFloodDetector`. |
| `http2.zero_window_stall_seconds` | `u64` | Same, via `ZeroWindowStallDetector`. |

### QUIC / HTTP/3

| Key | Spec type | Status |
|-----|-----------|--------|
| `quic.listen_address` | `String` | Not wired. `QuicEndpoint::server_on_loopback` exists (Pillar 3a) for tests; binary integration is Pillar 3b. |
| `quic.zero_rtt_enabled` | `bool` | Spec; `ZeroRttReplayFilter` exists but is not invoked. |
| `quic.alt_svc_advertise` | `bool` | Pillar 3c. |

### Pool

| Key | Spec type | Status |
|-----|-----------|--------|
| `pool.per_peer_max` | `usize` | Default hard-coded in `TcpPool::new` via `PoolConfig::default` (value from PROMPT.md §21). Config key is Pillar 2b. |
| `pool.total_max` | `usize` | Same. |
| `pool.idle_timeout_seconds` | `u64` | Same. |
| `pool.max_age_seconds` | `u64` | Same. |

### DNS

| Key | Spec type | Status |
|-----|-----------|--------|
| `dns.positive_ttl_cap_seconds` | `u64` | `ResolverConfig::default` (Step 5c); config key is Pillar 2b. |
| `dns.negative_ttl_seconds` | `u64` | Same. |
| `dns.refresh_interval_seconds` | `u64` | Same. |

### PROXY protocol

| Key | Spec type | Status |
|-----|-----------|--------|
| `proxy_protocol.accept_v1` | `bool` | Parser in `crates/lb-core` accepts v1; listener wiring is Pillar 3b. |
| `proxy_protocol.accept_v2` | `bool` | Same. |
| `proxy_protocol.send_outbound_v2` | `bool` | Outbound send is not yet implemented. |

### Observability

| Key | Spec type | Status |
|-----|-----------|--------|
| `observability.prometheus_bind` | `String` | No `/metrics` endpoint ships; see `METRICS.md`. Config key is Pillar 3b. |
| `observability.log_level` | `String` | Controlled via `RUST_LOG` env var, not TOML. See `RUNBOOK.md`. |

## Reload semantics

The configuration watcher sends SIGHUP → `crates/lb-controlplane/src/lib.rs` re-reads, validates, and publishes via `ArcSwap<Config>` (ADR-0008). Reload behavior per key:

- **`restart`**: a socket bind change. Requires process restart or a graceful handover (ADR-0009; FD-passing is deferred to Pillar 3b follow-up).
- **`SIGHUP`**: applied atomically via `ArcSwap` swap. The backend list, weights, and (eventually) detectors' thresholds reload on the next acquired listener reference. Existing in-flight connections see the old value; new connections see the new one. This is tested by `tests/reload_zero_drop.rs`.

## Validation

`lb_config::validate_config` runs after parse:

- Listener addresses must parse as `SocketAddr`.
- Backend `weight` must be `>= 1`.
- Listener backend list must be non-empty (or the listener is skipped at runtime with a warning).
- Further validation (TLS cert/key existence, QUIC config coherence) will land with the corresponding feature.

## Invalid-config behavior

On parse error, `lb` exits with a non-zero status and the parse error on stderr (`context`-wrapped via `anyhow`). On validation error, same. On SIGHUP validation failure, the old configuration stays in effect and the error is logged — zero-drop.
