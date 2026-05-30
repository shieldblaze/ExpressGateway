# S16 B6 — wiring map (main.rs + config + metrics + docs)

Read-only research result. Citations are to the s16-lead worktree.

## 1. Listener wiring (crates/lb/src/main.rs)
- H3 termination: `spawn_quic` (main.rs:1000-1026) → `quic_listener_params_from_config`
  (main.rs:476-489) → `QuicListener::spawn`. The router is built in
  `listener.rs:230-249` with `raw_quic_backend: None` at **listener.rs:247** ← the
  Mode B slot (set `Some(RawBackend{pool, addr, sni})`).
- Mode A passthrough: `spawn_passthrough` (main.rs:1034-1069), separate `[passthrough]`
  block; metrics threaded at main.rs:1053-1056.
- **Mode B**: gate a Mode-B variant on the QUIC listener when
  `quic.mode_b_backend.is_some()`; build the `RawBackend` + pool, pass via
  `RouterParams.raw_quic_backend`. Reuse `QuicListener::spawn` (the router already
  threads the field, B1).

## 2. Config (crates/lb-config/src/lib.rs)
- `ListenerConfig` (l.551): `protocol: String` (l.573), `quic: Option<QuicListenerConfig>` (l.578).
- `QuicListenerConfig` (l.902-918): cert/key/retry_secret/idle/payload. **Add**
  `mode_b_backend: Option<ModeBBackendConfig>` (`addr: SocketAddr`, `sni: String`,
  `tls_verify_peer: bool`, `tls_ca_path: Option<String>`).
- Validate in `validate_quic_listener` (l.1561+): addr resolves, SNI non-empty,
  CA present if verify_peer.

## 3. Metrics (mirror crates/lb-observability/src/passthrough_metrics.rs)
- `PassthroughMetrics` struct (l.18-39), `register(&MetricsRegistry)` (l.55-89),
  threaded `params.metrics = Some(...)` in main.rs. Idempotent register.
- New `ModeBMetrics` (`crates/lb-observability/src/modeb_metrics.rs`), `quic_modeb_*`:
  `connections` (gauge), `streams_relayed_total`, `bytes_relayed_total`,
  `datagrams_relayed_total` + `datagrams_dropped_total` (B4),
  `reset_propagated_total` (B3), `upstream_dial_errors_total`,
  `upstream_handshake_timeout_total`. Thread `Option<ModeBMetrics>` through
  `RawBackend`/the raw actor (None = no-op in tests, mirrors passthrough).

## 4. Upstream pool template (main.rs:730-793 build_h3_upstream_pool)
- Factory: `quiche::Config::new` + `set_application_protos(&[b"lb-quic"])` (l.768) +
  verify_peer/CA (l.771-775) + tuning; `QuicUpstreamPool::new(QuicPoolConfig::default(), factory)`.
- Mode B: build the same; ALPN default is overridden per-dial by `dial_dedicated`
  (mirrors client ALPN). verify_peer/CA from `mode_b_backend`.

## 5. 0-RTT (owner ruling: reject v1)
- `build_server_config` (listener.rs:346-369) has **NO `enable_early_data`** —
  keep it. 0-RTT-rejection test (B6): per `s16-quiche-api-notes.md` §0-RTT —
  client `set_session(prior_ticket)` + attempt `stream_send` while
  `is_in_early_data()`; assert with the server NOT enabling early data the client's
  `is_in_early_data()` flips false before `is_established()` (early data not acted on,
  completes 1-RTT). (Sharper than a CRYPTO-frame assertion.)

## 6. Docs (DEPLOYMENT.md)
- Add a QUIC-listener-modes section (H3 / Mode A passthrough / Mode B) + a Mode B
  config example + the 0-RTT-rejected-in-v1 (+1 RTT on resumption) operator note.

## B6 build order
config struct + validate → ModeBMetrics → main.rs pool+listener wiring + metrics
register → real-wire test (reuse s16_b1_two_connections harness, add datagrams
once B4 lands) + two-connections proof + 0-RTT-rejection test → DEPLOYMENT.md.
