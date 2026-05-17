# Plan for ROUND8-L7-06 — `keepalive_requests` count cap (Pingora 0.8.0 catch-up; nginx default 100)

Finding-ref:   ROUND8-L7-06 (medium, status: Open)
Files touched:
  - `crates/lb-config/src/lib.rs`             (`max_keepalive_requests`)
  - `crates/lb-l7/src/h1_proxy.rs`            (per-connection counter in `ProxyService`; `Connection: close` + `disable_keep_alive`)
  - `crates/lb-l7/src/h2_proxy.rs`            (per-connection lifetime stream cap → GOAWAY)
  - `crates/lb-observability/src/prometheus_exposition.rs`  (new counter)
  - `config/default.toml`                     (document the knob)
  - new test file: `crates/lb-l7/tests/round8_keepalive_count_cap.rs`

Approach (≤500 words):

References: **Pingora 0.8.0 CHANGELOG** added `keepalive_requests` as a
new feature (Cloudflare hit the operational pain). **nginx default** is
`keepalive_requests 100`. `ref-l7` handoff Top-10 #1 marks this as the
industry-standard floor.

Config:
```toml
[runtime]
max_keepalive_requests = 100   # 0 disables the cap (transparent-pass mode)
```

**H1 implementation:**

`ProxyService` clones into hyper's `Service::call` once per request,
but the *connection* identity needs a shared per-connection counter.
Wrap `ProxyService` with a connection-scoped wrapper at the
`serve_connection` site:

```rust
struct ConnectionScopedService {
    inner: ProxyService,
    requests_served: Arc<AtomicU32>,
    cap: u32,
    notify_close: Sender<()>,  // signals the connection driver to close
}
impl Service for ConnectionScopedService {
    fn call(&self, req) -> ... {
        let count = self.requests_served.fetch_add(1, SeqCst) + 1;
        let force_close = count >= self.cap && self.cap > 0;
        let inner = self.inner.clone();
        let notify = self.notify_close.clone();
        Box::pin(async move {
            let mut resp = inner.handle(req, ...).await?;
            if force_close {
                resp.headers_mut().insert(CONNECTION, HeaderValue::from_static("close"));
                let _ = notify.send(()).await;
            }
            Ok(resp)
        })
    }
}
```

The connection driver loop watches the `notify_close` channel; on
signal, after the current request completes, it calls
`hyper::server::conn::http1::Connection::graceful_shutdown` (already
wired by REL-2-02).

**H2 implementation:**

Hyper exposes `max_concurrent_streams` (we already set 256) but no
*lifetime* stream cap. Mirror the per-connection counter pattern:
- `ProxyService::call` for H2 increments a lifetime-stream counter
  on the connection-scoped state.
- On reaching the cap, emit GOAWAY via hyper's
  `graceful_shutdown()` (no new stream IDs accepted; in-flight
  streams complete normally).

**Prometheus surface:**
- `lb_keepalive_terminated_by_count_cap_total{listener, protocol}`

**Boundary disclosure:** the `serve_connection` wiring lives partly in
`lb-l7/src/h1_proxy.rs::serve_connection_with_cancel_sni` (already
div-l7's house). No `lb/src/main.rs` changes.

Reference pattern: nginx `ngx_http_keepalive_handler` increments a
per-connection counter checked against `keepalive_requests`. Pingora
0.8.0 `keepalive_requests` field on the upstream session.

Proof:
  - `round8_keepalive_count_cap::h1_101_requests_close_on_101st` — invariant: with `max_keepalive_requests = 100`, the 100th response carries `Connection: close` and the connection is closed by the server after that response flushes; the 101st request must dial a new connection.
  - `round8_keepalive_count_cap::h1_cap_zero_disables` — invariant: with cap = 0, 200 sequential requests on one connection all succeed; no `Connection: close` ever set by the cap (other paths may still set it).
  - `round8_keepalive_count_cap::h2_lifetime_streams_emit_goaway` — invariant: with `max_keepalive_requests = 50`, the 51st stream-attempt on a single H2 connection receives the GOAWAY frame's last-stream-id boundary; the client must dial a new H2 connection.
  - `round8_keepalive_count_cap::counter_increments_on_termination` — invariant: the new Prometheus counter advances exactly once per cap-triggered close.
  - `round8_keepalive_count_cap::cap_value_clamped_to_u32` — invariant: a configured value exceeding `u32::MAX` clamps to `u32::MAX` at config parse, not at runtime.

Risk / blast radius:
  - Lower default than the unbounded current behaviour. Operators
    with workloads that genuinely rely on long pipelines (e.g.
    server-sent-events long-poll clients reconnecting many times
    on one keepalive) need to raise the cap; document in the
    runbook.
  - The `notify_close` channel adds one `Sender` clone per
    connection. Acceptable overhead (channels are cheap; one per
    long-lived connection is bounded by the connection-count cap).

Cross-ref:
  - L7-15 (edge-defaults parity) — this knob is one row of the
    canonical edge defaults table; closing L7-06 also partially
    closes L7-15.
  - REL-2-02 (graceful shutdown wiring) — reuses the existing
    `graceful_shutdown` plumbing; no new connection-driver hooks
    required.

**Audit-failure-mode theme:**
  - **Theme 4 — Multi-validator audit handoff**: the Pingora 0.8.0
    feature landed after our Round 4 audit; the protocol validator
    re-walked the H2 settings table but not the keepalive table.
    Pingora's CHANGELOG was a primary input to ref-l7 in Round 8.
  - **Theme 2 — Operational-vs-laboratory test posture**: this
    bug only bites at long-lived connection counts and TLS-session
    age scales not exercised by single-instance test rigs.

Owner:           div-l7
Lead-approval: approved 2026-05-14 team-lead-r8
