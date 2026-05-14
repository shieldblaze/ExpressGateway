# Decision: H2 `max_concurrent_streams = 256`

**Status**: documented (closes the `max_concurrent_streams` row of ROUND8-L7-15)
**Round**: 8 (Phase D)
**Owner**: div-l7
**Reviewed-by**: team-lead (lead-decision R8-L-007 — bulk plan approval)
**Date**: 2026-05-14

## Decision

ExpressGateway's HTTP/2 server advertises `SETTINGS_MAX_CONCURRENT_STREAMS = 256`
on every accepted H2 connection. This is the value pinned by
`H2SecurityThresholds::default::max_concurrent_streams` in
`crates/lb-l7/src/h2_security.rs` (live constant: `256`).

The reference baselines diverge from us:

| Reference                                      | Default                                          |
| ---------------------------------------------- | ------------------------------------------------ |
| Envoy edge best-practice                       | 100                                              |
| nginx `http2_max_concurrent_streams`           | 128                                              |
| hyper (server builder unconfigured)            | unbounded (no `SETTINGS` frame sent)             |
| h2 (raw crate) default                         | unbounded                                        |
| ExpressGateway                                 | 256                                              |

## Rationale

1. **Pingora-class throughput**. ExpressGateway targets the
   Pingora-equivalent throughput envelope (workload mix: many small
   concurrent fetches over fewer long-lived connections). 100 streams
   on a single H2 connection serialises tail latency for clients that
   pipeline aggressively (CDN purge tools, gRPC streaming endpoints).
   256 doubles Envoy's headroom while remaining well below hyper's
   "unbounded" default.
2. **Per-connection DoS surface is bounded elsewhere**. The H2 stream
   explosion attacks (CVE-2023-44487 "Rapid Reset" / RUSTSEC-2024-0003)
   are bounded by `max_pending_accept_reset_streams` and
   `max_local_error_reset_streams`, both pinned to 100 per 10-s window.
   The concurrent-streams cap is not the primary DoS knob — the
   reset-window cap is. Raising concurrent streams from 100 to 256
   does not change the rapid-reset blast radius.
3. **Per-stream cost is bounded**. Each accepted stream is bounded by
   `max_header_list_size` (64 KiB) plus `max_send_buf_size` (64 KiB).
   At 256 concurrent streams the per-connection worst case is
   approximately `256 * 128 KiB = 32 MiB` plus the connection-level
   1 MiB receive window. That is acceptable on the connection-count
   envelope we target.
4. **Operator escape hatch already exists**. The setting is
   configurable via `[h2_security].max_concurrent_streams` in the
   listener config (see `H2SecurityConfig::max_concurrent_streams`
   in `crates/lb-config/src/lib.rs`). Operators with tighter DoS
   constraints can clamp to 128 or 100 without code changes.

## When to revisit

- If we add a per-IP or per-tenant connection-count cap that lets us
  amortise the 32 MiB worst-case across many connections, we can
  raise this further.
- If we ship a tenant-shared listener where one tenant's stream
  explosion would starve another's, we should drop to 128 and document
  the per-tenant share.
- If hyper or the h2 crate changes the per-stream memory cost, the
  arithmetic above changes and we re-derive.

## Implementation

Live constant: `crates/lb-l7/src/h2_security.rs` line 82
(`max_concurrent_streams: 256`). Applied to hyper via
`H2SecurityThresholds::apply` line 115. Pinned by
`crates/lb-config/tests/round8_compile_time_defaults.rs`.

## References

- Envoy edge best-practices guide
- nginx `http2_max_concurrent_streams` documentation
- CVE-2023-44487 / RUSTSEC-2024-0003 (h2 rapid-reset)
- hyper `Http2Builder::max_concurrent_streams` documentation
- `docs/edge-defaults.md`
- `audit/round-8/findings/ROUND8-L7-15.md`
