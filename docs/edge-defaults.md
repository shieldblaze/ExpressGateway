# Edge-defaults parity table (ROUND8-L7-15)

This document records ExpressGateway's edge-listener defaults against the
canonical reference baselines (Envoy edge best-practices and nginx
defaults). It exists so the operator (and the next auditor) can see at a
glance where ExpressGateway agrees with the industry baseline and where
it deliberately diverges. Each row cites the live constant in code so
divergence detection has a single source of truth.

Closure note: this is the meta-finding for L7-15. The constituent rows
that required code changes ship under L7-05, L7-06, L7-07, L7-12, and
L7-13. The table below is the inventory.

## Comparison table

| Knob                                         | ExpressGateway default                              | Envoy edge best-practice         | nginx default                      | Source of truth (this repo)                                              | Status                                                                |
| -------------------------------------------- | --------------------------------------------------- | -------------------------------- | ---------------------------------- | ------------------------------------------------------------------------ | --------------------------------------------------------------------- |
| H2 `max_concurrent_streams`                  | 256                                                 | 100 (`max_concurrent_streams`)   | 128 (`http2_max_concurrent_streams`) | `crates/lb-l7/src/h2_security.rs` (`H2SecurityThresholds::default`)      | Deliberate divergence; see `audit/decisions/h2-edge-streams.md`.       |
| H2 `initial_stream_window_size`              | 65 535 (RFC 9113 default)                           | 65 536                           | n/a (handled by nginx http_v2 mod) | `crates/lb-l7/src/h2_security.rs::initial_stream_window_size`            | Matches RFC default; matches Envoy within one byte.                   |
| H2 `initial_connection_window_size`          | 1 MiB                                               | 1 MiB                            | n/a                                | `crates/lb-l7/src/h2_security.rs::initial_connection_window_size`        | Matches.                                                              |
| H2 `max_header_list_size`                    | 64 KiB                                              | 8 KiB (envoy default)            | 4 KiB (`large_client_header_buffers`) | `crates/lb-l7/src/h2_security.rs::max_header_list_size`                  | Higher than Envoy; matches Pingora.                                   |
| H2 `keep_alive_interval` / `_timeout`        | 30 s / 30 s                                         | 1 h ping / 10 s timeout (varies) | n/a                                | `crates/lb-l7/src/h2_security.rs` (`keep_alive_interval`, `_timeout`)    | More aggressive than Envoy default; targets zero-window stall.        |
| H2 `max_pending_accept_reset_streams`        | 100                                                 | 100 (`CVE-2023-44487` patch)     | n/a                                | `crates/lb-l7/src/h2_security.rs` (via `lb_h2::DEFAULT_SETTINGS_MAX_PER_WINDOW`) | Matches.                                                              |
| H1/H2 `headers_with_underscores`             | reject (default, per L7-05)                         | `REJECT_REQUEST` (edge guide)    | `underscores_in_headers off` (drop) | `crates/lb-config/src/lib.rs::HeaderUnderscorePolicy::default`           | Matches Envoy edge stance.                                            |
| H1 `keepalive_requests` count cap            | `[runtime].max_keepalive_requests` (default **100**, `0` disables) | per-listener via `[runtime]`     | 100 (`keepalive_requests`)         | `crates/lb-l7/src/h1_proxy.rs` (`with_max_keepalive_requests` + conn-scoped counter; cap-th response → `Connection: close` + graceful_shutdown) | Proposed-Fix (L7-06, cherry-pick `0575e5ac`): H1 half landed at nginx-parity default. H2 lifetime-stream-cap→GOAWAY half deferred to a documented follow-up (H2 already enforces `max_concurrent_streams`). |
| H1/H2 header-read timeout                    | 10 s (`header_timeout_ms`)                          | `request_headers_timeout = 10s`  | `client_header_timeout 60s`        | `crates/lb-config/src/lib.rs::default_header_timeout_ms`                 | Matches Envoy; tighter than nginx.                                    |
| H1/H2 body-read timeout                      | 30 s (`body_timeout_ms`)                            | `idle_timeout = 1h`              | `client_body_timeout 60s`          | `crates/lb-config/src/lib.rs::default_body_timeout_ms`                   | Tighter than both references; deliberate.                             |
| Total request lifetime                       | 60 s (`total_timeout_ms`)                           | `request_timeout = 5s` (envoy)   | n/a (no built-in equivalent)       | `crates/lb-config/src/lib.rs::default_total_timeout_ms`                  | Looser than Envoy; covers long-poll WebSocket bridging.               |
| Path normalisation                           | off (transparent pass-through)                      | `normalize_path = on`            | `merge_slashes off` (configurable) | `audit/decisions/path-normalisation.md`                                  | Deliberate divergence; routing pillar will re-evaluate.               |
| H3 `MAX_DATA` / per-stream credit            | quiche defaults (`16 MiB` conn, `1 MiB` stream)     | n/a (Envoy QUIC alpha)           | n/a                                | `crates/lb-quic/src/lib.rs` (quiche default-config)                      | Conservative; tracked separately.                                     |

## Drift detection

`crates/lb-config/tests/round8_compile_time_defaults.rs` pins each row
of the table to the live constant via a literal assertion. A future
upgrade that drifts a default surfaces as a CI failure here. The test
intentionally hard-codes the values rather than re-deriving them — the
point is to force the auditor to re-check the table when the constant
moves.

## How to land a change to a default

1. Update the constant in the listed source-of-truth file.
2. Update the corresponding row in this table.
3. Update the assertion in `round8_compile_time_defaults`.
4. Add a one-line CHANGELOG entry citing the reference that motivated
   the change.

The three-step lockstep is intentional: a default change without a
table+test update fails CI; a table change without a test update fails
CI; a test change without a table update is flagged in code review by
the `docs/edge-defaults.md` doc-lint that ships under cross-ref
bundle B-2.

## References

- Envoy edge best-practices guide
  (`https://www.envoyproxy.io/docs/envoy/latest/configuration/best_practices/edge`)
- nginx `http_core` / `http_v2` module documentation
- Pingora `0.8.0` CHANGELOG (Cloudflare) — keepalive_requests
- `audit/decisions/path-normalisation.md`
- `audit/decisions/h2-edge-streams.md`
- `audit/round-8/findings/ROUND8-L7-15.md`
