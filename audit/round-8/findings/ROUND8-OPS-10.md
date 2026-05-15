### ROUND8-OPS-10 — Drain budget 10 s default is below the Pingora / HAProxy norm for long-lived gRPC/H2 streams; no per-listener override

Reference: `audit/round-8/research/pingora.md` architecture summary ("`EXIT_TIMEOUT = 300s` (default). `CLOSE_TIMEOUT = 5s`") + lesson 14 (H2 graceful = GOAWAY + drain in flight, then close); `audit/round-8/research/haproxy.md` lesson 20 (hot reload via master-worker socket inheritance — workers enter "soft stop", finish in-flight requests, then exit). Cloudflare published Pingora's 300-second EXIT_TIMEOUT specifically because in-flight long-lived H2 streams (live video, server-sent events, gRPC bidi) need that budget.
Our equivalent: `crates/lb-config/src/lib.rs:158-159` (`drain_timeout_ms` default 10000); `crates/lb-config/src/lib.rs:843-848` (validation range 100..=300_000 ms, i.e. 100 ms..5 minutes); `RUNBOOK.md:57` (default 10 000 ms documented). No per-listener override key.

Severity: medium
Status:   Verified-Fixed(verifier=verify, c26056b5)   <!-- [[listeners]].drain_timeout_ms override + effective_drain_timeout_ms resolver + validation 100..=300000 inherit-runtime (lib.rs:997,1214-1228); coordinator InFlightDrain uses max effective budget; RUNBOOK tuning section. Per-conn-await-own-budget is disclosed div-l7 follow-up. See audit/round-8/verify/ops.md. -->
Status (pre-verify):   Open

Divergence:
- Pingora ships `EXIT_TIMEOUT = 300s` as the default; HAProxy / nginx allow per-frontend `hard-stop-after` / similar. We ship 10 s as the default.
- 10 s is correct for short-request HTTP listeners. It is materially insufficient for:
  - Long-poll H1: typical 30-second pollers will be cut at 10 s, return early, and the client immediately reconnects to whichever pod the LB above sends it to.
  - gRPC bidi streaming: a streaming-RPC that runs for tens of seconds is severed mid-stream. The gRPC client sees `RST_STREAM(CANCEL)` and retries with a fresh stream id — but the request was *not* idempotent in many cases.
  - Server-Sent Events: same shape as long-poll, same outcome.
  - WebSocket upgrades: hyper's `Connection: close` after `graceful_shutdown` is correct, but a 10 s budget is below the typical WebSocket session lifetime; almost every WS is severed by drain rather than completed.
- The validation range 100..=300_000 ms (5 minutes) accommodates the Pingora default, but our shipped *default* is the short-request value. There is no per-listener override — every listener shares the gateway-level value.

Impact:
- A roll of an ExpressGateway pod hosting an SSE or gRPC streaming workload severs every in-flight stream at the 10-second mark. The Pingora default would let those streams complete naturally.
- The runbook (`RUNBOOK.md:188`) does call out "Check `[runtime].drain_timeout_ms` vs your typical request latency p99" — but only as a triage step under `LbShutdownAborted`. It does not tell the operator "for long-stream workloads, set this to 300_000".

Recommendation:
1. Document the per-workload default selection table in `CONFIG.md`:
   - Short-request HTTP/H1/H2: 10_000 ms (current default).
   - Streaming HTTP/SSE/long-poll: 60_000 ms.
   - gRPC bidi / WebSocket: 300_000 ms (Pingora default).
2. Add a per-listener override: `[listeners.X].drain_timeout_ms` overrides the gateway-level value. A streaming listener can be 5 minutes while a regular listener stays at 10 s.
3. Add `shutdown_drain_seconds` histogram (see ROUND8-OPS-03) labeled by `listener` so the operator can pick the right value empirically.
4. Add `LbShutdownTruncatedStreams` alert in `RUNBOOK.md`: `rate(shutdown_aborted_connections_total[1h]) > 0 AND any listener has http_version="h2"` — points to under-budget drain for streaming workloads.
5. Document in `RUNBOOK.md` Drain section the Pingora reference (300 s) so operators know the upper bound is intentional, not a bug.

Notes:
- This is a "different design, possibly wrong" finding in the round-8 taxonomy. Our value is defensible for the most-common case (request/response HTTP) but does not accommodate the workloads three production references explicitly designed for. Operators who deploy us for those workloads will hit it.
