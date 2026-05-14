# Round 2 — Reliability Findings (`rel`)

Repo: `/home/ubuntu/Code/ExpressGateway` @ commit `ac58f61` (`main`).
Inputs: `audit/reliability/round-1-inventory.md`, `audit/CROSS-REVIEW-SYNTHESIS-r1.md`.

IDs are stable across rounds. Line numbers are HEAD of `main` at the time
of Round 1. Severities follow the audit charter:
- **critical** = immediate prod blocker; on-call cannot mitigate without
  code or doc change.
- **high** = will degrade an SLO under expected load or makes an
  incident materially worse.
- **medium** = operability gap that limits debuggability or correctness
  of a runbook step.
- **low/info** = polish.

Status is `Open` for every finding; the team-lead flips status in Round
6.

---

### REL-2-01 — Doc/code drift: RUNBOOK and DEPLOYMENT describe flows that do not exist
Severity: high
Status:   Open
Location: `RUNBOOK.md:25-57`, `RUNBOOK.md:89`, `RUNBOOK.md:135-150`, `DEPLOYMENT.md:11`, `DEPLOYMENT.md:32-65`, `DEPLOYMENT.md:117`, `DEPLOYMENT.md:147-157`

Description: Multiple operator-facing flows in `RUNBOOK.md` and
`DEPLOYMENT.md` describe behaviour that is not present in source. Per
Round-1 inventory §0 the drift set is:
- `SIGHUP` reload via `ConfigManager` + `ArcSwap` (`RUNBOOK.md:37`):
  no `SignalKind::hangup` handler in `crates/lb/src/main.rs`;
  `lb-controlplane::ConfigManager` is never instantiated from `lb`.
- `KillMode=mixed` + `TimeoutStopSec=30` graceful drain
  (`RUNBOOK.md:25`): actual budget is ~2.5 s (`main.rs:1049,1060`).
  See REL-2-02.
- `ArcSwap<TlsStore>` cert rotation (`RUNBOOK.md:49-57`,
  `DEPLOYMENT.md:123-125`): no `TlsStore` exists. See REL-2-03.
- `tracing-subscriber::fmt::format::json` "one-line flip"
  (`RUNBOOK.md:89`, `DEPLOYMENT.md:147`): the binary does not select
  the JSON formatter. See REL-2-06.
- Systemd unit `ExecStart=/usr/local/bin/lb` (`DEPLOYMENT.md:44`),
  artefact path `target/release/lb` (`DEPLOYMENT.md:11`), and
  `strings /usr/local/bin/lb` (`RUNBOOK.md:161`): the binary is
  named `expressgateway` (`crates/lb/Cargo.toml:[[bin]] name`,
  `docker/Dockerfile` line 34). The unit will fail on install. See
  REL-2-13.
- Log lines `reload applied` (`DEPLOYMENT.md:157`), `pool probe
  discarded` (`RUNBOOK.md:146`), `liveness probe`
  (`RUNBOOK.md:86`): grep for each string returns zero hits in
  `crates/`. Operators following these RUNBOOK filters get no
  results.

Impact: An on-call engineer following the documented playbook at 3 a.m.
runs commands that silently no-op (SIGHUP), look at log filters that
match nothing (`reload applied`), and trust a drain budget (30 s) that
is 12× longer than reality. The runbook actively misleads.

Reproduction: Walk the runbook top-to-bottom against the binary; the
first hot-reload step exits 0 with no state change.

Recommendation: Treat docs as code. Either
1. **Strike** every drift line and replace with the current restart-only
   behaviour, or
2. **Implement** the documented mechanism (preferred for SIGHUP and
   cert rotation; see REL-2-02 / REL-2-03).

Each individual drift item is a separate finding below; this entry is
the meta-finding for the doc-rewrite task. The lead's Round-1
synthesis pre-approved this as the umbrella ID.

Cross-ref: `sec` §5.1 (cert rotation), `code` Q-CODE-1-04/05 (SIGTERM
drain), `proto` (GOAWAY).

---

### REL-2-02 — SIGTERM is not a drain; in-flight frames severed at ~2.5 s
Severity: critical
Status:   Open
Location: `crates/lb/src/main.rs:1032-1060`

Description: The shutdown path is:
1. `JoinHandle::abort()` on each TCP listener (line 1038) — kills the
   accept loop but **does not propagate** to per-connection tasks
   spawned at `main.rs:1126`. Those tasks are not tracked in any
   `JoinSet` and hold no `CancellationToken`.
2. `admin_cancel.cancel()` — clean for the admin listener.
3. QUIC listeners get `shutdown()` and a fixed `timeout(Duration::from_secs(2))`
   wait at lines 1049-1057.
4. `tokio::time::sleep(Duration::from_millis(500))` at line 1060.
5. Process exits — every per-connection task is severed.

Observed total wall-clock drain: ~2.5 s. `RUNBOOK.md:25` claims 30 s
via systemd. The 30 s only fires if the binary keeps the process alive,
which it does not.

Impact: Any TCP/H1/H1s/H2 connection in `copy_bidirectional` or mid-L7
frame at SIGTERM is RST'd. H2 clients see TCP reset instead of GOAWAY +
graceful stream close (RFC 7540 §6.8). For a 500 ms drain window an
in-flight H2 stream carrying a large response body is truncated; the
client's retry budget burns on every deploy. Replicas that observe
SIGTERM during a rolling restart simultaneously slam their fleet of
connections — the load balancer above ExpressGateway sees a thundering
herd of resets every deploy.

Reproduction:
1. `curl --http2 https://gw/long_response &` (200 MB body)
2. `systemctl stop expressgateway`
3. Wireshark / `ss -tn` shows TCP RST mid-body, not GOAWAY +
   FIN/last-stream.

Recommendation: Lead pre-approved decision applied here.
1. **Drain budget**: 10 s default, configurable via new
   `[runtime].drain_timeout_ms` key in `lb-config`. Hard cap; abort
   fallback only on overflow.
2. **Drain ordering** (single new `shutdown()` function in
   `crates/lb/src/main.rs`):
   a. Flip `/readyz` to 503 (see REL-2-04). Wait 1 s (one external
      LB probe period) so the upstream stops sending new requests.
   b. For each listener: stop accepting (cancel via a per-listener
      `CancellationToken` instead of `JoinHandle::abort`).
   c. Per-protocol drain — `code` co-owns the cancellation-token
      plumbing; `proto` co-owns H2 GOAWAY + H3 CONNECTION_CLOSE:
      - H1: emit `Connection: close` on the next response.
      - H2: emit GOAWAY with `last_stream_id`, then close after the
        last open stream ends or budget elapses.
      - H3 / QUIC: emit `CONNECTION_CLOSE` frame; quiche flush.
      - Plain TCP: half-close client→backend, drain backend→client.
   d. After `drain_timeout_ms`, the abort fallback fires (preserves
      today's behaviour as a hard cap, not the primary path).
3. **In-flight registry**: replace the bare `tokio::spawn` at
   `main.rs:1126` with `JoinSet` per listener, owned by
   `ListenerState`. Drain `JoinSet::shutdown()` during step (c).
4. **Metric**: emit `shutdown_drain_seconds` histogram +
   `shutdown_aborted_connections_total` counter (so operators can see
   when the abort fallback fires).

Cross-ref: `code` owns the `CancellationToken` plumbing (per Round-1
synthesis T2); `proto` owns GOAWAY / CONNECTION_CLOSE emission.

---

### REL-2-03 — TLS certificate rotation is documented but not implemented
Severity: critical
Status:   Open
Location: `crates/lb/src/main.rs:214`, `crates/lb/src/main.rs:611`, `RUNBOOK.md:49-57`, `DEPLOYMENT.md:123-125`

Description: `RUNBOOK.md:49-57` and `DEPLOYMENT.md:123-125` describe a
hot cert reload via SIGHUP + `ArcSwap<TlsStore>`. In source:
- `arc-swap = "1"` is declared in `Cargo.toml:61` but is not imported by
  `lb` (`grep -rn ArcSwap crates/lb/` returns nothing). The dep is
  workspace-declared and unused on this path.
- `build_tls_stack` (`main.rs:214`) and `build_h1s_tls_stack`
  (`main.rs:611`) build a `TlsAcceptor` once from filesystem paths and
  hand the resulting `Arc<TlsAcceptor>` into `ListenerState`. There is
  no swap surface.
- The ticket-key rotator (`crates/lb-security/src/ticket.rs::TicketRotator`)
  rotates the **session ticket key** only — not the certificate or
  private key. The rotator is correct in scope but the runbook
  conflates it with cert rotation.

Impact: The only way to roll a cert is a process restart, which goes
through REL-2-02's broken drain — every cert roll is a fleet-wide
truncation event. Cert pinning rotations, ACME renewals, and emergency
revocations cannot be done without a planned drop.

Reproduction: `mv /etc/expressgateway/tls/cert.pem cert.pem.new && kill
-HUP $(pidof expressgateway)` — no log line, no cert refresh, next TLS
handshake still serves the old chain.

Recommendation:
1. Introduce `Arc<ArcSwap<TlsConfigBundle>>` shared by every TLS
   listener (`TlsAcceptor` should be reconstructed per accept from the
   current swap; alternatively use rustls's `ResolvesServerCert` trait
   with an `ArcSwap` inside). `sec` should sign off on the cert-resolver
   approach.
2. On SIGHUP (see REL-2-05 wiring): reload cert + key from the same
   filesystem paths, validate (chain depth, key match, expiry), and
   `store` into the `ArcSwap`. Validation failure leaves the old bundle
   live.
3. In-flight handshakes use the snapshot they loaded; new handshakes
   pick up the new bundle.
4. Emit `tls_cert_reload_total{result}` and
   `tls_cert_not_after_seconds{listener}` gauges so the runbook has an
   alert on expiry.

Cross-ref: `sec` joint owner (cert-validation correctness, key-match
check, expiry gauge), per Round-1 synthesis T3.

---

### REL-2-04 — `/healthz` is unconditional 200; no liveness/readiness/startup split
Severity: high
Status:   Proposed-Fix(7108d9e)
Location: `crates/lb-observability/src/admin_http.rs:55`

Description: The admin listener serves a single `/healthz` endpoint
returning `200 ok\n` unconditionally (`admin_http.rs:55`). There is no
distinction between process liveness, listener readiness, or backend
health. Specifically:
- During listener bind failure for one of N listeners, the process
  still returns 200 while serving partial traffic.
- During SIGTERM drain (REL-2-02), `admin_cancel` fires at
  `main.rs:1041` and the admin listener **stops accepting** — an
  upstream LB polling `/healthz` sees connection-refused rather than a
  503 "draining" signal.
- The K8s probe model (livenessProbe, readinessProbe, startupProbe)
  cannot be wired correctly with a single endpoint.

Impact: K8s + external LBs cannot do safe rolling restarts. Either they
restart on the wrong condition (kill on first listener flap) or they
fail to drain (no readiness flip before SIGTERM). On-call cannot
distinguish "process up, listeners down" from "all healthy" via the
exposed surface.

Reproduction: Misconfigure one listener so it fails to bind; `curl
/healthz` returns 200.

Recommendation:
1. Add `/livez` — returns 200 if the tokio runtime is up. Equivalent to
   today's `/healthz` semantics. Keep `/healthz` as alias for
   back-compat.
2. Add `/readyz`:
   - 200 only if **every configured listener** is currently bound and
     accepting (track per-listener `Arc<AtomicBool>` `accepting` flag,
     set true after first successful `accept()`, set false at the start
     of drain).
   - And at least one backend per route is currently healthy (gated
     once REL-2-08 lands).
   - During SIGTERM drain step (a) of REL-2-02, `accepting=false`
     before listeners stop. `/readyz` flips to 503 with a 1 s settle
     window before per-listener cancel fires.
3. Add `/startupz`: 200 once the binary has bound every listener and
   completed initial DNS resolution. K8s uses this to delay
   liveness/readiness probes during boot.
4. Keep all three on the admin listener (loopback-bound). `sec` to
   confirm no auth needed at loopback.

Cross-ref: REL-2-02 (drain ordering), `sec` (loopback assumption).

---

### REL-2-05 — `HealthChecker` and `ConfigManager` are dead code from the binary's view
Severity: high
Status:   Open
Location: `crates/lb-health/src/lib.rs:54` (`HealthChecker`); `crates/lb-controlplane/src/lib.rs:156` (`ConfigManager`); `crates/lb-cp-client` (workspace member, unused)

Description: Three crates declared as workspace members host types that
are never linked from the `lb` binary:
- `HealthChecker` (`crates/lb-health/src/lib.rs:54-62`) is referenced
  only by `crates/lb-health/src/lib.rs`'s own tests (lines 121, 127,
  137, 146, 164, 171). `grep -rn HealthChecker crates/` outside that
  file: zero hits. The round-robin balancer keeps every backend in
  rotation regardless of liveness; a dead peer stays in service until
  per-request `connect(2)` fails.
- `ConfigManager` (`crates/lb-controlplane/src/lib.rs:156`) is similar
  — only `crates/lb-controlplane/src/lib.rs` tests (lines 337-447)
  instantiate it. No SIGHUP plumbing exists (see REL-2-01).
- `lb-cp-client` is a workspace member; `cargo tree -p lb` does not
  include it (per Round-1 inventory §F-10).

`code` independently confirmed via `cargo machete` that
`lb-controlplane` and `lb-health` are flagged unused in dependent
crates (Round-1 synthesis T9 reference).

Impact: Three documented capabilities (active health-checking,
SIGHUP-reload, control-plane partition tolerance) are missing from
production. A dead backend silently drops every Nth request (where N is
the rotation length). A misconfig push has no rollback. A control-plane
outage is undefined — there is no client to outage.

Reproduction:
1. Backend health: stop one of two configured backends. Observe that
   50% of requests hit the dead peer until `connect(2)` fails per
   request; no rotation removal.
2. SIGHUP: edit config, `kill -HUP`. No state change.

Recommendation:
1. **Wire `HealthChecker`** into the balancer:
   - Spawn one task per backend with `HealthChecker` cadence from
     config (default 5 s).
   - Each `pick()` filters by `is_healthy()` first; if no healthy
     backend, return a typed error that the listener path maps to 503
     (H1/H2) or `CONNECTION_REFUSED` (plain TCP).
   - Emit `backend_health_state{listener,backend}` gauge (0/1) and
     `backend_health_transitions_total{listener,backend,to}` counter.
2. **Wire `ConfigManager`** via SIGHUP:
   - Install `signal(SignalKind::hangup())` in `async_main` next to
     `shutdown_signal`.
   - On signal: re-read config path, call
     `lb_config::validate_config`, and on success do an atomic swap
     (likely via REL-2-03's `ArcSwap` infrastructure for cert, plus a
     listener-set diff for added/removed listeners). On failure, log
     and keep the old config.
3. **Decision required (Round 3)**: keep `lb-cp-client` or delete?
   The synthesis defers; flagging here for the planning round. If
   kept, wire it next to `ConfigManager`.

Cross-ref: `code` `cargo machete` reference; `sec` (validation safety
of reloaded config; reject downgrades).

---

### REL-2-06 — Logs are plain text despite docs claiming JSON
Severity: medium
Status:   Proposed-Fix(15c9018)
Location: `crates/lb/src/main.rs:931-936`

Description:
```rust
tracing_subscriber::fmt()
    .with_env_filter(...)
    .init();
```
No `.json()` call; default `fmt()` is human-readable text. The
workspace declares `tracing-subscriber = { version = "0.3", features =
["json", "env-filter"] }` (`Cargo.toml:59`) so the feature is
available — it is just not selected.

`DEPLOYMENT.md:147` claims "JSON to stdout via
`tracing_subscriber::fmt().with_env_filter(...)`" — false. `RUNBOOK.md:89`
calls out the gap honestly but no code change followed.

Impact: Log shippers (`vector`, `fluent-bit`, `journalctl -o
json`) cannot extract structured fields without an additional parsing
stage. Hot-path log lines that include `peer=<addr>` and
`error=<...>` get glommed into a free-form string, breaking grafana log
panels and alert filters.

Reproduction: `expressgateway --config ... | head -1` → text, not JSON.

Recommendation:
1. Default to JSON in production: `.json().flatten_event(true)`. Gate
   text formatter behind a `LB_LOG_FORMAT=text` env var for local
   development.
2. Drop a `version` field and a `host`/`pid` field via
   `with_current_span(true)` so downstream shippers can index on them.
3. Cross-ref with REL-2-07: each request span should serialize as a
   field set, not a free-form `info!` line.

---

### REL-2-07 — No distributed tracing: no `traceparent` propagation, no per-request span
Severity: high
Status:   Proposed-Fix(1d462c7)
Location: `crates/lb-l7/src/h1_proxy.rs`, `crates/lb-l7/src/h2_proxy.rs`, `crates/lb-l7/src/grpc_proxy.rs`, `crates/lb-quic/src/lib.rs`

Description: `grep -rn 'traceparent\|opentelemetry\|info_span\|tracing::Span' crates/`
returns zero hits across the L7 stack. Concretely:
- No W3C Trace Context (`traceparent`, `tracestate`) header
  injection/forwarding on H1, H2, gRPC, or H3.
- No per-request `tracing::info_span!` opened in the L7 proxies. The
  closest thing is connection-level `tracing::info!` lines.
- No upstream-timing child span. We cannot answer "where did the
  500 ms go: client→LB, LB→backend, backend→LB, LB→client?" from logs
  or traces.

Impact: When a user reports "5% of requests are slow", on-call has no
trace-id pivot to a downstream service. Every cross-service
investigation requires log-correlation by timestamp + 5-tuple, which
is approximate and breaks under retries. This is a hard prerequisite
for any non-trivial production deployment.

Reproduction: Send a request with `traceparent:
00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01`. Examine the
upstream backend — the header is forwarded byte-for-byte (because
nothing strips hop-by-hop except minimally) but no child span is
emitted; LB latency is invisible to the trace consumer.

Recommendation:
1. Add `opentelemetry` + `tracing-opentelemetry` workspace deps.
   Optional feature: `otel` (default off only if MSRV concerns;
   otherwise default on).
2. In each L7 entry point, open `tracing::info_span!("lb.request",
   listener, route, http.version, http.method, http.target)` and
   capture status + bytes in fields on exit.
3. **Header policy**:
   - If client supplied `traceparent`: parse, validate, use as parent;
     forward to upstream.
   - If not: synthesize a new trace id; **inject** `traceparent` on
     the upstream request (configurable per-route — some operators
     do not want LB-originated traces).
4. Child span for upstream dial + first-byte: timestamps allow the
   classic "is the LB slow or is the backend slow" answer.
5. Export via OTLP/grpc on the admin listener bind (separate from
   `/metrics`).

Cross-ref: REL-2-06 (JSON logs carry the span id), `proto` (header
strip rules for hop-by-hop vs trace headers).

---

### REL-2-08 — Per-listener / per-backend RED labels missing
Severity: medium
Status:   Proposed-Fix(551d470)
Location: `crates/lb/src/main.rs:1186-1196` (counter_vec registration); `crates/lb-observability/src/lib.rs`

Description: The exposed HTTP RED metrics are:
- `http_requests_total{version, status_class}` — 4 series.
- `http_request_duration_seconds{version}` — 3 series.
- Pool / DNS / bytes counters: no labels.

There is no `listener`, `route`, or `backend` label on any RED metric.
With ≤4 series total, a dashboard cannot answer "is the regression on
listener `:443` or `:8443`?" or "is backend `10.0.0.5` failing while
the others are healthy?".

Impact: Dashboards built off these metrics are uselessly coarse — they
show "the LB has errors" but never localise to a listener or backend.
SLO burn alerts cannot be sliced. An operator at 3 a.m. has no
prometheus pivot.

Reproduction: scrape `/metrics`; observe ≤7 series across all the
hot-path families.

Recommendation:
1. Add `listener` label (bind address as string), `backend` label
   (resolved socket addr), and where possible `route` label (host or
   path bucket; empty if not enabled). Cardinality budget:
   listeners×backends×status_class — for typical 5×10×4 = 200 series
   per metric, well within prom's comfort.
2. Re-scope `http_requests_total` and `http_request_duration_seconds`
   to be **per-request**, not per-connection, by moving the counter
   increment into `lb-l7` next to the response builder. Today
   `main.rs:1186` runs per connection and double-counts keep-alive
   reuse incorrectly.
3. Add `backend_requests_total{listener, backend, status_class}` for
   upstream-side observability.
4. Add `connections_inflight{listener}` gauge (track via
   `Arc<AtomicI64>` per listener, increment on accept and decrement
   on task exit). This is the saturation signal REL-2-09 needs.

Cross-ref: REL-2-09 (saturation), `code` (per-request hook placement
inside `lb-l7`).

---

### REL-2-09 — Unbounded `tokio::spawn` per TCP accept; missing `accept_inflight` saturation metric and alert
Severity: critical
Status:   Open
Location: `crates/lb/src/main.rs:1099-1126`

Description: The accept loop:
```rust
loop {
    let (client_stream, client_addr) = match listener.accept().await { ... };
    // ...
    tokio::spawn(async move { ... });
}
```
There is no per-listener semaphore, no `max_inflight` gate, no shed
policy. Under a connection storm the binary OOMs by task before it OOMs
by bytes; long before that, tokio scheduler fairness degrades and
latency for healthy connections collapses.

QUIC's `max_connections: 100_000` (`crates/lb-quic/src/listener.rs:245`)
is the **only** in-flight cap in the codebase.

This finding is **joint with `code`** per the Round-1 synthesis T7.
`rel` owns the saturation metric + the runbook entry + the alert; `code`
owns the `Semaphore` plumbing.

Impact: A trivial SYN-flood (or a slow misbehaving client fleet) drives
the per-listener task count to OOM, and there is no metric for the
operator to see it coming.

Reproduction: `hping3 -S -p 443 -i u500 <lb>` and measure RSS growth.

Recommendation (rel-owned slice):
1. Emit `accept_inflight{listener}` gauge — derive from a per-listener
   `Arc<AtomicI64>` incremented at spawn, decremented at task exit.
2. Emit `accept_total{listener}` counter (already partially covered by
   `connections_total`; re-label per listener — see REL-2-08).
3. Emit `accept_shed_total{listener,reason}` counter when the
   semaphore (added by `code`) returns "would block". Reasons:
   `inflight_cap`, `pool_exhausted`, `draining`.
4. **Default cap**: `max_inflight = 50_000` per listener
   (configurable via `[listeners.*].max_inflight`); on overflow,
   immediately close the accepted socket (TCP: drop;
   TLS: socket close before handshake; H1/H2: 503 Service Unavailable
   for already-accepted but un-handshook keep-alive reuse).
5. **Runbook entry**: alert
   `LbAcceptSaturation: accept_inflight / max_inflight > 0.8 for 2m`
   → check upstream traffic source, look for slowloris/synflood, scale
   replicas, raise `max_inflight` if backend pool can absorb.

Cross-ref: `code` owns the `Semaphore` itself; REL-2-10 (EMFILE),
REL-2-11 (`spawn_blocking`).

---

### REL-2-10 — `accept(2)` errors are a tight loop; no backoff, no `accept_errors_total`
Severity: critical
Status:   Open
Location: `crates/lb/src/main.rs:1099-1106`

Description:
```rust
let (client_stream, client_addr) = match listener.accept().await {
    Ok(conn) => conn,
    Err(e) => {
        tracing::warn!("accept error: {e}");
        continue;
    }
};
```
On a transient error this is fine. On a persistent error like EMFILE
(file-descriptor exhaustion — easy to hit when the backend pool is
under pressure and idle FDs are not reclaimed fast enough) the
`accept()` future returns immediately with the same error; `continue`
spins at CPU speed, the `warn!` line fills logs at gigabytes per
minute, and the binary does no useful work. EAGAIN/EINTR/ECONNABORTED
have similar shapes.

Joint with `code` per Round-1 synthesis T7. `rel` owns the
`accept_errors_total{kind}` metric + runbook + alert; `code` owns the
backoff implementation.

Impact: EMFILE escalation is a known production fire-drill mode. With
the current loop it goes from "slow degradation" to "log volume DoS +
CPU pegged" in seconds. SREs lose log retention budget on the same
incident.

Reproduction: `ulimit -n 64`; start the binary; open 64 idle
connections; the 65th accept returns EMFILE and the loop spins.

Recommendation (rel-owned slice):
1. Emit `accept_errors_total{listener, kind}` counter. Kinds:
   `emfile`, `enfile`, `eintr`, `econnaborted`, `eagain`, `other`
   (mapped from `io::Error::raw_os_error()`).
2. Demote `tracing::warn!` to a single `error!` per backoff window
   (rate-limited). Use `tracing::warn_once!` pattern or roll our own.
3. **Runbook entry**: alert `LbAcceptErrors: rate(accept_errors_total[1m]) > 10`
   → check `lsof -p $(pidof expressgateway) | wc -l` vs the soft fd
   limit; raise `LimitNOFILE`; investigate FD leak in pool.
4. Cross-ref `code`: implement an exponential backoff
   (start 1 ms, cap 1 s) on persistent error kinds; reset on the next
   success.

Cross-ref: `code` owns backoff loop.

---

### REL-2-11 — `spawn_blocking` for upstream connect on the global blocking pool
Severity: high
Status:   Open
Location: `crates/lb-io/src/lib.rs::Runtime::connect`; `crates/lb-io/src/pool.rs:167`; `crates/lb/src/main.rs:1238`

Description: Pool `acquire` on a cold peer dials via
`tokio::task::spawn_blocking` wrapping `std::net::TcpStream::connect_timeout`
(`Runtime::connect`). The blocking call has no caller-supplied timeout
bound, so on a SYN black-hole the default kernel timeout (~75 s on
Linux) keeps the blocking-pool slot pinned.

The default tokio blocking pool is 512 slots. Under a fleet-wide
backend outage:
- Every accept that triggers a cold-dial parks one blocking worker.
- After 512 such accepts (trivially reachable for any non-toy load),
  the blocking pool is full.
- `fs::*`, `spawn_blocking` for cert loads, and any other
  blocking-pool consumer block forever.

This is **structurally a sharp edge**, not a bug per se. Flagged here
for `code` to evaluate alternatives in Round 3.

Impact: Single-backend outage cascades into total LB unavailability via
blocking-pool starvation. Symptom is a hang, not an error — operator
sees `accept_total` flat, no errors, no logs.

Reproduction: blackhole one backend with `iptables -A OUTPUT -p tcp
--dport <backend> -j DROP`; saturate the LB with 600 concurrent
clients; observe new client connects hang.

Recommendation:
1. **Bounded connect timeout** — caller passes explicit timeout
   (default 2 s, configurable per-backend). This is independent of
   the larger choice.
2. **Alternative 1 (preferred)**: switch to non-blocking
   `tokio::net::TcpStream::connect(addr).timeout(Duration)`. No
   blocking pool involved. Round-3 plan should evaluate behavioural
   parity vs Pingora.
3. **Alternative 2**: keep `spawn_blocking` but use a **dedicated**
   bounded executor (`tokio::runtime::Handle::block_on` is not the
   answer; rather, a `tokio::task::LocalSet` on a dedicated
   single-thread runtime, or a custom `rayon`-style pool). This
   isolates connect-blocking from cert-load-blocking.
4. **Metric**: `backend_connect_seconds{listener,backend}` histogram +
   `backend_connect_errors_total{listener,backend,kind}` counter
   (kinds: `timeout`, `refused`, `unreachable`, `other`).

Cross-ref: `code` evaluates the alternative for Round-3 plan
(per-synthesis disagreement #4).

---

### REL-2-12 — CONNTRACK saturation has no userspace metric or alert
Severity: high
Status:   Proposed-Fix(365815f)
Location: `crates/lb-l4-xdp/src/loader.rs`; `crates/lb/src/xdp.rs`; XDP `STATS` map (kernel side)

Description: The XDP data plane (Pillar 4b) maintains a CONNTRACK map
(`HashMap`, fixed-size — `sec` S-2 / `ebpf` §2 separately flag the
type choice). When the map is full, new flows cannot be installed and
must either fall through or drop.

Today the kernel-side `STATS` map exists but is **not** read by
userspace. `grep -rn STATS crates/lb-l4-xdp/src/loader.rs` returns
zero hits. The userspace metric `xdp_conntrack_full_total` does not
exist.

This is a tri-owner finding per Round-1 synthesis T4: `ebpf` owns the
map-type fix and the userspace export wiring; `sec` owns the
attack-evidence; `rel` owns the userspace metric + alert + runbook.

Impact: Adversarial flood (or organic spike) fills CONNTRACK; the LB
silently degrades. No metric, no alert, no log line — operator finds
out from user reports.

Reproduction: synflood at >65 k unique source 5-tuples per minute;
CONNTRACK fills; no observable signal in `/metrics`.

Recommendation (rel-owned slice):
1. **Add userspace samplers** (sibling to the existing pool/DNS
   sampler at `main.rs:892`): every 1 s read the STATS map and emit:
   - `xdp_conntrack_full_total` (Counter, monotonic) — number of
     "insert failed because map full" events.
   - `xdp_conntrack_entries_current` (Gauge) — current cardinality.
   - `xdp_packets_total{action}` (Counter) — `forwarded`, `dropped`,
     `passed_to_kernel`.
   - `xdp_acl_deny_total` (Counter).
2. **Runbook entry**: alert
   `XdpConntrackPressure: xdp_conntrack_entries_current / map_capacity > 0.85 for 5m`
   → investigate flow churn, raise map capacity (requires reload),
   inspect source-IP distribution for attack signal.
3. **Cross-coordinate with `ebpf`** on the actual STATS map schema
   and userspace read API (Round-1 inventory open-question EBPF-1-1).

Cross-ref: `ebpf` owns map-fix + export; `sec` owns attack-evidence
in S-2.

---

### REL-2-13 — Per-CPU STATS map never exported (kernel-side counters invisible)
Severity: medium
Status:   Proposed-Fix(a500ff7)
Location: `crates/lb-l4-xdp/ebpf/src/main.rs` (STATS map definition); `crates/lb/src/xdp.rs` (loader hook)

Description: Sibling to REL-2-12. The XDP program maintains a
`PER_CPU_ARRAY` STATS map (per `ebpf` Round-1 inventory §7 — to be
confirmed) for fast in-kernel counters. The userspace loader never
fetches this map. None of the existing Prometheus metrics are sourced
from it.

This finding is the kernel→userspace export wiring; REL-2-12 covers
the specific saturation-signal subset. Splitting them because the
total kernel STATS surface is broader than CONNTRACK-full.

Impact: Hot-path counters (packets dropped by XDP, packets passed up
to the kernel stack, ACL denies, parse errors) are inaccessible from
`/metrics`. The dataplane is opaque.

Reproduction: Run any XDP-bound test and scrape `/metrics`; no
`xdp_*` family is present.

Recommendation:
1. In the existing 1 s sampler (`main.rs:892`), add a `PerCpuArray`
   read of every STATS index and sum across CPUs into a single
   counter.
2. Naming convention: `xdp_packets_total{action}`, `xdp_errors_total{kind}`,
   `xdp_bytes_total{direction}`.
3. Coordinate with `ebpf` for the exact index→kind mapping. The
   userspace mapping must live in `lb` (not `lb-l4-xdp`) so that
   tightening the kernel program does not break consumer code.

Cross-ref: `ebpf` #7 (Round-1 inventory).

---

### REL-2-14 — Binary name mismatch: systemd unit and runbook reference `/usr/local/bin/lb`; binary is `expressgateway`
Severity: low
Status:   Open
Location: `DEPLOYMENT.md:11`, `DEPLOYMENT.md:44`, `RUNBOOK.md:161`; `crates/lb/Cargo.toml:[[bin]] name`; `docker/Dockerfile:34`

Description: The binary built from `crates/lb` is named
`expressgateway` (`docker/Dockerfile` line 34 confirms
`COPY --from=builder /app/target/release/expressgateway
/usr/local/bin/expressgateway`). The systemd unit in `DEPLOYMENT.md:44`
declares `ExecStart=/usr/local/bin/lb /etc/expressgateway/lb.toml`. The
runbook (`RUNBOOK.md:161`) suggests `strings /usr/local/bin/lb | grep
ExpressGateway`. Both paths 404 on a real install.

`DEPLOYMENT.md:11` also references `target/release/lb` — also wrong.

Impact: First-time operator deploying via the unit hits
`status=203/EXEC` immediately. The runbook diagnostic that follows
also fails. Triage takes minutes that the on-call doesn't have.

Reproduction: Copy the unit verbatim → `systemctl start
expressgateway` → fails.

Recommendation: Replace every `/usr/local/bin/lb` and `target/release/lb`
with `/usr/local/bin/expressgateway` / `target/release/expressgateway`.
Add a CI doc-lint check that greps both files for paths not in
`docker/Dockerfile`.

No code change; doc-only. Ship the systemd unit as an actual file
(`packaging/expressgateway.service`) tested by a smoke job — see Round
3 plan.

---

### REL-2-15 — No panic hook installed; `panic = "unwind"` in release means silent task death
Severity: high
Status:   Open
Location: `crates/lb/src/main.rs:920-927`; `Cargo.toml:162` (`[profile.release]` lacks `panic = "abort"`)

Description: The release profile (`Cargo.toml:162-166`) does not set
`panic = "abort"`, so Rust's default `panic = "unwind"` applies. In a
tokio multi-thread runtime, a panic inside a spawned task unwinds that
task only — the runtime catches it, logs to **stderr** via tokio's
default panic handler, and the process keeps running. The operator
sees nothing in structured logs (REL-2-06 issue compounds).

There is no `std::panic::set_hook(...)` call anywhere in the binary
(`grep -rn 'set_hook\|panic_hook' crates/` returns zero hits).
`RUNBOOK.md:148` claims libs are panic-free per Cloudflare's 2025 rule,
but that does not prevent the binary itself (or unwrap-in-test-only
paths leaking) from panicking.

Impact: A latent bug that panics on one in N requests is *invisible*.
Symptom: per-request error rate is real but unattributable. SLO burn
without a log signal. Cross-ref `sec`: a panic-on-malicious-input is
also a covert availability bug.

Reproduction: Inject a `panic!` into one branch of `lb-l7`; observe
that `/metrics` shows nothing, the only signal is a stderr line in
journalctl (not the structured log pipeline).

Recommendation:
1. Install `std::panic::set_hook` early in `async_main` (before any
   spawn). Hook emits a structured `tracing::error!(panic = true,
   message = ..., location = ..., backtrace = ...)` log line and
   increments `panics_total{location}` counter.
2. **Decision (Round-3 plan)**: pick one:
   - **(a)** Keep `unwind` + the hook; rely on tokio recovering each
     task (current behaviour minus the invisibility).
   - **(b)** Set `panic = "abort"` in `[profile.release]` and rely on
     systemd `Restart=on-failure` to bring the process back. Loses
     the in-flight tasks but guarantees a clean exit + the panic is
     visible at the kernel level.
   `sec` should weigh in on which side has the smaller attack surface
   (option (b) eliminates a class of unwind-safety bugs).
3. Alert: `LbPanics: rate(panics_total[5m]) > 0` → page.

Cross-ref: `code` (panic-hook plumbing), `sec` (unwind-vs-abort
attack-surface trade).

---

## Severity roll-up

| Severity | IDs |
|----------|-----|
| critical | REL-2-02, REL-2-03, REL-2-09, REL-2-10 |
| high     | REL-2-01, REL-2-04, REL-2-05, REL-2-07, REL-2-11, REL-2-12, REL-2-15 |
| medium   | REL-2-06, REL-2-08, REL-2-13 |
| low      | REL-2-14 |

## Cross-team dependencies

| Finding | Joint owner | What they own |
|---------|-------------|---------------|
| REL-2-02 | `code`, `proto` | `CancellationToken` plumbing (code); H2 GOAWAY + H3 CONNECTION_CLOSE (proto) |
| REL-2-03 | `sec` | Cert validation, key match, expiry semantics |
| REL-2-05 | `code` | `machete`-flagged unused deps; SIGHUP wiring approach |
| REL-2-08 | `code` | Per-request hook placement inside `lb-l7` |
| REL-2-09 | `code` | `Semaphore` implementation; this finding owns metric + alert |
| REL-2-10 | `code` | Backoff loop; this finding owns metric + alert |
| REL-2-11 | `code` | Round-3 plan: blocking-pool vs non-blocking connect |
| REL-2-12 | `ebpf`, `sec` | Map-type fix (ebpf); attack-evidence (sec) |
| REL-2-13 | `ebpf` | STATS map schema + userspace read API |
| REL-2-15 | `code`, `sec` | Panic-hook plumbing; unwind-vs-abort trade |
