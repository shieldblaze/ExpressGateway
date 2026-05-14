# Deferral note for ROUND8-L7-06 — `keepalive_requests` count cap

Finding-ref: ROUND8-L7-06 (medium, status: Open)
Owner: div-l7
Date: 2026-05-14
Status: **deferred to Wave 2b (post drain-coordinator landing)**

## Reason

The L7-06 plan as approved requires a per-connection counter wired
between `ProxyService::call` and the `hyper::server::conn::http1::
Connection` driver, with a notify channel so the connection driver
invokes `graceful_shutdown` after the Nth response flushes. The
connection-scoped wrapper has to coordinate with two concurrent
agents that are also touching the connection-driver perimeter:

1. **div-ops drain coordinator** is editing `crates/lb/src/main.rs`
   for OPS-06, which owns the SIGTERM-driven `graceful_shutdown`
   call-site. The L7-06 wiring needs to slot the cap-driven
   `graceful_shutdown` call into the same `serve_connection_with_
   cancel_sni` body that OPS-06 modifies. Landing L7-06 in Wave 1
   would force two concurrent edits to the `tokio::select!` block
   inside `serve_connection_with_cancel_sni`, with no clean
   cherry-pick conflict-resolution path.
2. **H2 GOAWAY emission for the H2 cousin** requires changing the
   hyper `http2::Builder` chain inside the same H2 serve loop the
   OPS-06 drain path also touches.
3. **Prometheus counter** registration is in scope under the brief,
   but the registration call-site is in `crates/lb/src/main.rs`
   (alongside every other `lb_observability::Registry::register_*`
   call), which is the file div-ops is editing concurrently.

## Wave 1 carries vs Wave 2b

The Wave-1 carry list was tagged for low-coordination plans where
the implementation could land entirely inside lb-l7 / lb-h1 / lb-h2
/ lb-config without main.rs entanglement. L7-06's *implementation*
fits that constraint at the field/builder level (the
`ConnectionScopedService` wrapper lives in `h1_proxy.rs`), but the
*activation path* (graceful_shutdown wiring + Prometheus
registration + H2 GOAWAY mux) all sit at the same call-site
boundary as OPS-06's drain coordinator.

Per the task brief: "if any requires `crates/lb/src/main.rs`, STOP
that one and report — it'll move to Wave 2b after div-ops's drain
coordinator lands." L7-06 is the one of the four Wave-1 carries
that hits this gate.

## What landed in Wave 1 instead

- L7-15 (edge-defaults parity table + ADR + pinning test) — landed.
- L7-10 (H1 take-and-discard docs + H2 broad-eviction contract) — landed.
- L7-05 (header_underscore_policy with reject default) — landed.

## What still needs to land in Wave 2b

1. `max_keepalive_requests` field on `RuntimeConfig` (lb-config).
2. `with_max_keepalive_requests` builder methods on `H1Proxy` and
   `H2Proxy` (lb-l7) plus the per-connection counter inside
   `serve_connection_with_cancel_sni`.
3. `Connection: close` injection on the Nth H1 response + graceful
   shutdown after flush.
4. H2 GOAWAY emission on lifetime-stream cap reached.
5. `lb_keepalive_terminated_by_count_cap_total{listener, protocol}`
   Prometheus counter, registered alongside the OPS-06 drain
   metrics in `crates/lb/src/main.rs`.
6. Tests: `round8_keepalive_count_cap::{h1_101_requests_close_on_101st,
   h1_cap_zero_disables, h2_lifetime_streams_emit_goaway,
   counter_increments_on_termination, cap_value_clamped_to_u32}`.

## Pin in the edge-defaults table

The L7-15 doc explicitly lists this row as **Open — pending L7-06**.
That's the audit trail until the Wave 2b commit closes both
findings together.

## Cross-ref

- `audit/round-8/fixes/ROUND8-L7-06.md` (the original approved plan)
- `audit/round-8/findings/ROUND8-L7-06.md`
- `docs/edge-defaults.md` row 8 (status column: "Open: tracked under
  L7-06; deferred to Wave 2b.")
- `audit/round-8/fixes/OPS-06.md` (the drain coordinator div-ops
  is landing concurrently)
