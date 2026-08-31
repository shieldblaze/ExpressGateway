# S47 — Reliability / Observability review

Scope: `crates/lb-observability/src/**`, `crates/lb-l7/src/trace_ctx.rs`,
the `Err` arms of `lb-l7/{h1,h2}_proxy.rs` + `lb-quic/{h3_bridge,conn_actor}.rs`
+ `lb/src/main.rs`, `crates/lb-soak/**`, and the operational artifacts
(`docs/guide/RUNBOOK.md`, `docs/guide/METRICS.md`, `packaging/expressgateway.service`,
`docker/Dockerfile`, `scripts/soak/*`).

Method: read-and-reason only (no cargo — 2 vCPU / 7 GB / 11 GB box). Every
finding cites `file:line` and quotes the live code. Prior art was read first;
items already documented as pending/limitations are marked `ALREADY-KNOWN` and
are not counted as findings.

**Line numbers are pinned to `27307be0`.** The branch is shared and moving —
`crates/lb/src/main.rs` drifted ~31 lines mid-review. Every citation below was
re-resolved against that commit; if a number no longer lands, grep the quoted
code, which is verbatim.

Severity is rated by **incident impact**: "an operator cannot diagnose this in
production" is treated as a real defect.

---

## Summary table

| ID | Sev | Location | Claim |
|----|-----|----------|-------|
| REL-01 | CRITICAL | `crates/lb/src/main.rs:2608` | Admin listener is cancelled *before* the drain coordinator runs — `/readyz` never serves 503, `/livez` goes dark mid-drain, and every drain metric is written after the scrape endpoint is closed |
| REL-02 | CRITICAL | `packaging/expressgateway.service:32` | Shipped unit is `Type=notify`; the binary never calls `sd_notify` — systemd start times out and `Restart=on-failure` loops forever |
| REL-03 | HIGH | `crates/lb/src/main.rs:3245` | `status_class` is derived from `result.is_ok()`, not the HTTP status — `LbReq5xx` reads zero during a total backend outage |
| REL-04 | HIGH | `crates/lb/src/main.rs:87-100` | The panic hook aborts 50 ms after bumping `panic_total`; the `LbPanic` alert can essentially never fire, and RUNBOOK asserts the opposite failure mode |
| REL-05 | HIGH | `packaging/expressgateway.service:37` | `ExecReload` sends SIGUSR1 (cert rotation); RUNBOOK + DEPLOYMENT.md both document SIGHUP — `systemctl reload` silently does not apply config changes |
| REL-06 | HIGH | `scripts/soak/soak-verdict.sh:52-57` | Release soak gate's panic check cannot match anything the logs contain, and load ok/err counts are never gated — an all-errors run PASSES |
| REL-07 | HIGH | `crates/lb-observability/src/log.rs:73-86` | Logging is a synchronous blocking write to stdout with no sampling and no non-blocking writer — a stalled log consumer blocks data-path workers |
| REL-08 | HIGH | `crates/lb-observability/src/label_budget.rs:154` | `EnforcedLabelBudget` — the *only* runtime cardinality guard — is dead code, called from nowhere in production |
| REL-09 | HIGH | `crates/lb-observability/src/admin_http.rs:183-212` | `/metrics` exposition runs on the data-path runtime with no concurrency cap, no timeout, and no connection limit |
| REL-10 | HIGH | `crates/lb/src/main.rs:3084,3263` | `http_request_duration_seconds` measures connection lifetime against 10 s-max buckets — `LbReqDuration` is permanently true on any keep-alive listener |
| REL-11 | MEDIUM | `crates/lb/src/main.rs:1988-2018` | The whole G5 passive-health metric family is absent from METRICS.md and RUNBOOK — `backend_ejections_suppressed_total` ("we know it's failing and still route to it") has no alert |
| REL-12 | MEDIUM | `crates/lb-soak/src/timeseries.rs:335` | Soak DRIFT requires `rel_growth > band` **AND** `monotone_frac >= 0.60`; a jittery slow leak escapes as BOUNDED, and the robust statistic (slope) is computed then discarded |
| REL-13 | MEDIUM | `crates/lb-soak/src/timeseries.rs:236-260` | `CounterMustBeZero` with **zero samples** returns BOUNDED and the note "stayed zero across the run" — the analyzer asserts a fact it never measured |
| REL-14 | MEDIUM | `crates/lb-soak/src/procstat.rs:36-41` | `count_fds` returns `0` on an unreadable `fd/` dir — the documented connection-leak discriminant reads a flat zero and verdicts BOUNDED |
| REL-15 | MEDIUM | `crates/lb-soak/src/sampler.rs:35-40` | `scrape()` collapses connect-timeout / 403 / non-2xx / parse failure into an empty `MetricSet`; no failed-scrape counter exists |
| REL-16 | MEDIUM | `crates/lb-observability/src/lib.rs:386-400` | `check_cardinality()` runs only when a new metric *family* is registered — it is blind to the label-value explosion it exists to catch |
| REL-17 | MEDIUM | `crates/lb-quic/src/**` | H3 has no W3C trace-context handling at all; no crate emits a child span for backend selection or upstream timing |
| REL-18 | MEDIUM | `crates/lb-l7/src/trace_ctx.rs:140-146` | `synth_trace_id` is seeded from a non-reserving `SPAN_COUNTER.load()` — concurrent root requests get **identical trace-ids** |
| REL-19 | MEDIUM | `crates/lb-observability/src/admin_http.rs:193-199` | Admin accept errors log at `debug` with no backoff — under EMFILE the diagnostic listener busy-spins a core and says nothing at default level |
| REL-20 | MEDIUM | `crates/lb-quic/src/h3_bridge.rs:944-952` | `Err(_)` discards the upstream body error; `RespAbort::{BadHead,ChunkedDecode}` each fold ≥2 operationally opposite causes into one label |
| REL-21 | MEDIUM | `crates/lb-observability/src/xdp_metrics.rs:38-41` | `xdp_sampler_errors_total` has no `kind` label although RUNBOOK and `CANONICAL_LABELS` both declare one; nothing diffs the live registry against the canonical table |
| REL-22 | MEDIUM | `crates/lb/src/main.rs:2671` | `shutdown_aborted_connections_total` is unlabelled, but `LbShutdownTruncatedStreams` is `sum by (listener)` |
| REL-23 | MEDIUM | `crates/lb-observability/src/log.rs:93` | `Err(_) => Err(TracingError::AlreadyInitialised)` discards the real `try_init` error — two causes, one wrong message |
| REL-24 | MEDIUM | `crates/lb-quic/src/h3_bridge.rs:1240-1244` | H3→H1 request-write failure returns a 502 with **no log line at all** |
| REL-25 | LOW | `crates/lb-observability/src/probes.rs:38` | Probe body token is `booting`; RUNBOOK documents `starting` |
| REL-26 | LOW | `crates/lb/src/main.rs:177-205,215-259` | `CertMetrics::register` / `ReloadMetrics::register` use `.ok()?`; one failure silently disables the whole family, unlogged |
| REL-27 | LOW | `crates/lb-observability/src/tracing_propagation.rs:171-173` | `tracestate` 32-entry limit not enforced; an oversize value is dropped with no log/metric |
| REL-28 | LOW | `crates/lb-l7/src/trace_ctx.rs:134-138` | `traceparent_raw` is destructured away — absent / malformed / future-version headers are three causes with one indistinguishable outcome |
| REL-29 | LOW | `Cargo.toml` profile + `crates/lb/src/main.rs:75` | `strip = "symbols"` makes `Backtrace::force_capture()` unsymbolized — RUNBOOK asks for an artifact the release binary cannot produce |
| REL-30 | LOW | `packaging/expressgateway.service:22-24` | Unit header cites two CI gates (`systemd-analyze-security`, doc-lint directive parity) that do not exist |
| REL-31 | LOW | `crates/lb/src/main.rs:2012-2018` | `backend_health_status{listener,backend}` series are never removed on reload — stale rows report "healthy" for backends no longer in the pool |
| REL-32 | INFO | `crates/lb-soak/src/bin/eg-soak.rs:167-208` | `eg-soak` exits 0 on DRIFT (the shell gate compensates; direct invocation does not) |
| REL-33 | INFO | `docs/guide/RUNBOOK.md:411-414` | `LbXdpConntrackFull` diagnose steps reference `xdp_conntrack_entries_current` / `_capacity`, which are registered nowhere |
| REL-34 | INFO | `crates/lb-observability/src/quic_modeb_metrics.rs:18-20` | `quic_modeb_streams_active` is last-writer-wins across concurrent actors — documented in code, absent from METRICS.md |
| REL-35 | INFO | `crates/lb-observability/src/admin_http.rs:201-210` | No admin connection cap and no `header_read_timeout` — admin fds share the process budget with the data path |

---

## CRITICAL

### REL-01 — the drain sequence closes the probe endpoint before it flips readiness

`crates/lb/src/main.rs:2606-2610`:

```rust
    // Cancel the admin listener BEFORE the coordinator so it does not serve `/readyz` Ready during
    // the settle window.
    admin_cancel.cancel();

    let report = shutdown.run_drain(spec).await;
```

`mark_draining` — the closure that actually flips readiness — is stored in the
spec at `main.rs:2598-2601` and is invoked *inside* `run_drain`:

```rust
        mark_draining: Some(Box::new(move || {
            tracing::info!("entering drain — flipping /readyz to 503");
            probes_for_mark.set_draining();
        })),
```

`serve_with_auth`'s accept loop returns on the token
(`crates/lb-observability/src/admin_http.rs:185-192`), dropping the
`TcpListener` — the socket closes, so probes get **connection refused**, not 503.

Three consequences, all operational:

1. **`/readyz` never serves 503.** The entire `readiness_settle_ms` design
   (default 11 000 ms, with a full "Tuning `readiness_settle_ms`" section in
   RUNBOOK arguing the upstream LB must *observe* the 503) rests on the endpoint
   being alive during the settle window. It is not. RUNBOOK drain step 2 —
   "External LBs scraping `/readyz` see 503 on the next probe (typically ≤ 10 s)"
   — is false; and the log line `entering drain — flipping /readyz to 503`
   announces a state change no one can observe.
2. **`/livez` goes dark at the same instant.** `crates/lb-observability/src/probes.rs:16-17`
   states the invariant explicitly: *"Draining: `/readyz` 503 to stop new traffic,
   but `/livez` stays 200 **or K8s kills the pod mid-drain**."* Closing the listener
   defeats it. With a common `livenessProbe: {periodSeconds: 5, failureThreshold: 2}`
   the kubelet SIGKILLs the container ~10 s into an 11 s settle window — before a
   single in-flight connection has been drained. The gateway's own code documents
   the hazard and the drain sequence walks into it.
3. **Every drain metric is written to a registry that can no longer be scraped.**
   `shutdown_drain_seconds_{global,listener}` (`main.rs:2873,2883`),
   `lb_drain_timeout_ms_listener` (`main.rs:2571`, populated only inside this
   post-signal block), `shutdown_listener_cancel_timeout_total` (`main.rs:2628`),
   `shutdown_inflight_drain_timeout_total` and the `inc_by(remaining)` on
   `shutdown_aborted_connections_total` (`main.rs:2671-2681`) are all recorded
   after `admin_cancel.cancel()`. RUNBOOK alerts `LbShutdownSlow`,
   `LbShutdownTruncatedStreams` and the triage-matrix commands
   (`curl 127.0.0.1:9090/metrics | grep shutdown_drain_seconds`) depend on data
   that is never exposed. `lb_drain_timeout_ms_listener` in particular is *only
   ever set during a drain*, so the `LbShutdownSlow` denominator
   `max by (listener) (lb_drain_timeout_ms_listener)` has no series in steady
   state and the expression yields nothing.

**Existing test coverage: none.** `crates/lb-observability/tests/health_endpoints.rs`
drives `serve_with_probes` with its own `CancellationToken` and proves the
`ProbeRegistry` state machine (`test_livez_readyz_startupz_states`), not the
binary's ordering. `tests/round8_drain_15case.rs` operates purely on
`lb_core::Shutdown` with a mock `DrainObserver` and never binds an admin
listener. `tests/reload_zero_drop.rs:366` only asserts `/readyz` flips to Ready
at boot. Nothing asserts "`/readyz` answers 503 while the settle window is open."

### REL-02 — the shipped systemd unit is `Type=notify` and nothing ever notifies

`packaging/expressgateway.service:31-42`:

```ini
[Service]
Type=notify
NotifyAccess=main
User=expressgateway
...
Restart=on-failure
RestartSec=5s
```

There is **no** `sd_notify`, `NOTIFY_SOCKET`, `libsystemd` or `sd-notify`
reference anywhere in the workspace (verified across `crates/`, `Cargo.toml`,
all crate manifests). With `Type=notify`, systemd blocks on `READY=1`; when it
never arrives, `TimeoutStartSec` (default 90 s) elapses, systemd declares the
start failed and SIGTERM/SIGKILLs the process, then `Restart=on-failure` +
`RestartSec=5s` restarts it — forever. The gateway is serving traffic
successfully the whole time and is killed every ~95 s.

The same file already knows this, 75 lines below (`:106-110`):

```ini
# Watchdog: requires sd_notify(WATCHDOG=1) from the process. Wave 2
# wires the heartbeat task; until then the unit may flap on
# WatchdogSec triggers, so this is commented out ...
#WatchdogSec=15s
```

`WatchdogSec` was correctly commented out for exactly this reason; `Type=notify`
was not. `docs/guide/DEPLOYMENT.md:51` renders the same unit as `Type=simple`,
which *would* work — the two artifacts disagree and the shipped one is the
broken one.

**Existing test coverage: none.** The unit header claims a CI gate
(`systemd-analyze security ... CI gate systemd-analyze-security enforces`) that
does not exist in any of `.github/workflows/{ci,scheduled,release}.yml`; and
`systemd-analyze security` scores hardening directives, it does not detect a
missing `sd_notify`.

---

## HIGH

### REL-03 — `status_class` measures connection teardown, not HTTP status

`crates/lb/src/main.rs:3244-3255`:

```rust
            if let Some(version) = http_version {
                let status_class = if result.is_ok() { "2xx" } else { "5xx" };
                let listener_label = st.listener_label.as_str();
                let route_label = "";
                if let Ok(v) = st.metrics.counter_vec(
                    "http_requests_total",
                    "HTTP requests terminated by the L7 proxy",
                    &["listener", "route", "version", "status_class"],
                ) {
                    v.with_label_values(&[listener_label, route_label, version, status_class])
                        .inc();
```

`result` is the return of `serve_connection_with_cancel`. `crates/lb-l7/src/h1_proxy.rs:390-392`
documents what that value means:

```rust
    /// Drive HTTP/1.1 server logic over `io` until the connection closes,
    /// bounded by [`HttpTimeouts::total`]. Per-request upstream errors become
    /// 502/504 responses and do NOT terminate the connection.
```

**Scenario.** Every backend is down. Every request returns a synthesized 502
(`h1_proxy.rs:787,798,1462,1482,...`). Each connection closes cleanly, so
`result.is_ok()` and the gateway increments
`http_requests_total{status_class="2xx"}`. The `LbReq5xx` alert —
`sum by (listener) (rate(http_requests_total{status_class="5xx"}[5m])) / sum(...) > 0.05`,
documented **Wired: yes** in `RUNBOOK.md:503-512`, and one of the four
"starter alerts" in `observability.md:113-119` — stays at **0.0** through a
total outage. The inverse also holds: a client that RSTs mid-connection is
counted as `5xx` although every response was a 200.

METRICS.md documents the *per-connection scoping* and the empty `route`
(`METRICS.md:54,75,118`) — those parts are ALREADY-KNOWN. What is documented
nowhere is that `status_class` is not derived from a status code at all. The
closest acknowledgement is a follow-up bullet at `METRICS.md:113`
("`http.requests.total` by actual response status"), which an operator reading
the alert catalog will never connect to "the 5xx alert is blind."

Related, same site: `http_version` is `Some(..)` only for the `H1` / `H1s`
listener arms (`main.rs:3128,3169,3181`). **H3/QUIC listeners emit no RED
metrics at all** — they are served by a different accept path, so an H3 outage
produces no movement in `http_requests_total` in either the numerator or the
denominator.

**Existing test coverage: no.** `tests/metrics_label_shape.rs` pins the *label
shape* only, and it hand-writes `"2xx"` / `"5xx"` values
(`metrics_label_shape.rs:48-51`); it never drives the emit site, so it cannot
observe that the value is Ok/Err-derived.

### REL-04 — `LbPanic` cannot fire, and RUNBOOK describes the opposite failure mode

`crates/lb/src/main.rs:87-100`:

```rust
        if let Some(c) = PANIC_TOTAL_COUNTER.get() {
            c.inc();
        } else {
            PANIC_TOTAL_FALLBACK.fetch_add(1, Ordering::Release);
        }
        tracing::error!(
            target: "panic",
            ...
            "process panic — aborting"
        );
        std::thread::sleep(Duration::from_millis(50));
        std::process::abort();
```

The abort is unconditional (no `cfg`), and the release profile is
`panic = "abort"` (root `Cargo.toml`). So the counter is incremented and the
process is gone ~50 ms later; the counter resets to 0 on restart. With a 15 s
scrape interval, `rate(panic_total[5m]) > 0` — documented **severity: page**,
**Wired: yes** (`RUNBOOK.md:284-286`) and listed in `observability.md:80` as
"**`panic_total` is flat at 0.** Any increase is a real defect — page on it" —
requires a scrape to land inside a 50 ms window. It will be flat at zero
forever, including after a panic.

RUNBOOK then tells the on-call the wrong thing twice:

- `RUNBOOK.md:293-294`: "If `panic = "unwind"` (current default), tokio caught
  it and the process kept running" — the release default is `abort`.
- `RUNBOOK.md:639`: "Returns; the runtime catches the unwind in the offending
  task and the rest of the process keeps serving." — the hook never returns.

`docs/arch/overview.md:267-272` states the correct behaviour, so the two doc
sets contradict each other and the incident-time document is the wrong one. An
on-call reading RUNBOOK during an incident concludes the process survived and
does not look for a restart or for dropped in-flight connections.

The signal that *would* work — process restart / `process_start_time_seconds`
discontinuity, or the structured `target: "panic"` log line — is not in the
alert catalog.

**Existing test coverage: partial and misleading.** `crates/lb-observability/tests/panic_total.rs`
and `main.rs:5371-5387` exercise the fallback-drain arithmetic under the *test*
profile (`panic = "unwind"`), i.e. exactly the profile that is not shipped.

### REL-05 — `systemctl reload` does not reload the config

`packaging/expressgateway.service:37`:

```ini
ExecReload=/bin/kill -USR1 $MAINPID
```

`RUNBOOK.md` "Configuration reload" states:

> `ExecReload=/bin/kill -HUP $MAINPID` dispatches **SIGHUP**, which runs the
> validate-first config hot reload (S37-C)

and `docs/guide/DEPLOYMENT.md:55` renders `ExecReload=/bin/kill -HUP $MAINPID`.

**Scenario.** An operator edits `[[listeners.backends]]` (a *swappable* field
per RUNBOOK) and runs `systemctl reload expressgateway`. SIGUSR1 fires the TLS
bundle reload instead. `reload_all_tls` re-reads the unchanged PEMs, succeeds,
and logs `REL-2-03 TLS cert reload succeeded` (`main.rs:311-317`). The operator
sees a successful "reload" in the journal; `config_reload_succeeded_total` never
moves; the backend pool is unchanged. RUNBOOK's own troubleshooting instructions
("Watch the logs after a reload: Success: `SIGHUP config reload pass complete`")
would show the absence, but only for an operator who already suspects the
problem.

RUNBOOK does hedge in a *different* section — the TLS rotation section says
"(or `systemctl reload expressgateway` if the unit's `ExecReload` is mapped to
SIGUSR1)" — so the runbook contains both answers and asserts the wrong one as
the primary procedure.

**Existing test coverage: none.** `scripts/ci/doc-lint.sh:51` contains exactly
one systemd assertion, a negative lint on `ExecStart=/usr/local/bin/lb`. The
unit header's claim that "the doc-lint job enforces that every directive named
in `DEPLOYMENT.md` appears here" is not implemented, and would not catch a
same-key/different-value divergence in any case.

### REL-06 — the release soak gate's failure detectors cannot match anything

`scripts/soak/soak-verdict.sh:50-57`:

```bash
  # awk, not grep: `grep panic_total[^0]` matches the trailing space on the
  # zero-valued metric line `panic_total 0` and over-counts.
  panics=0
  if [ -f "$log" ]; then
    panics=$(awk '
      /panicked/ { c++ }
      /panic_total[ {]/ { v=$NF; if (v ~ /^[0-9]+$/ && v+0 > 0) c++ }
      END { print c+0 }' "$log" 2>/dev/null)
```

`$log` is `$OUT/$sc.stdout.log` — `eg-soak`'s own stdout, redirected in
`scripts/soak/run-soak.sh:39-40`. Neither arm can match its contents:

1. **`/panicked/`** — the gateway child's stdout/stderr do **not** go there.
   `crates/lb-soak/src/gateway.rs:71-80`:

   ```rust
        let log = std::fs::File::create(&log_path)?;
        let log_err = log.try_clone()?;
        ...
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err))
   ```

   `log_path` is `<out>/<label>-work/gateway.log` (`eg-soak.rs:913`). Worse, even
   grepping *that* file for `panicked` would fail: `init_panic_hook` replaces the
   default panic hook, so Rust's stock `thread '...' panicked at ...` line is
   never printed — the only output is the structured
   `"process panic — aborting"` event, which does not contain the substring
   `panicked`.
2. **`/panic_total[ {]/`** — the heartbeat writes `" {g}={v:.0}"`
   (`crates/lb-soak/src/sampler.rs:103`), i.e. the log line contains
   `panic_total=0`. The regex requires a space or `{` immediately after
   `panic_total`; it sees `=`. Never matches.

So `panics` is structurally always `0`, and the gate reduces to
`overall == BOUNDED`.

**Second hole in the same gate: there is no throughput/liveness floor.**
`LoadStats` tracks `ok`/`err` (`crates/lb-soak/src/loadgen.rs:21-24`) and
`render_summary` prints them (`eg-soak.rs:1012-1015`), but `soak-verdict.sh`
never reads them, and `samples` is printed (`samples=${samples:-?}`) and never
compared. A scenario in which **every request failed** — `ok=0, err=4_000_000`
— has flat RSS and flat fds, verdicts BOUNDED, and PASSES the release gate.

**Existing test coverage: none** — the gate is shell; nothing tests the gate.

### REL-07 — logging is a blocking, unsampled, globally-serialized stdout write

`crates/lb-observability/src/log.rs:72-86`:

```rust
    let install_result = match format {
        LogFormat::Json => fmt()
            .json()
            .flatten_event(true)
            ...
            .try_init(),
```

`tracing_subscriber::fmt()` defaults to `io::stdout` as its `MakeWriter`. There
is no `tracing-appender` dependency anywhere in the workspace
(`crates/lb-observability/Cargo.toml` and the root manifest confirm), so there
is no non-blocking writer, and there is no sampler or rate limiter of any kind —
only `EnvFilter` level gating.

Two failure modes the on-call will meet:

1. **Disk full / wedged log shipper.** stdout is a pipe (docker `json-file`
   driver, a sidecar collector, a systemd `StandardOutput=file`). When the reader
   stalls, `write(2)` blocks with no timeout. The caller is a tokio worker
   thread. The runtime is a single shared `new_multi_thread` pool
   (`crates/lb/src/main.rs:2109-2112`) carrying the entire data path, so *n*
   blocked log writes wedge *n* workers. The lead's question — "on disk full,
   does logging block the data path?" — is answered yes.
2. **Log-rate amplification on the hot path.** `Stdout` is a `LineWriter` behind
   a global reentrant mutex: every worker serializes on one lock. The already-
   tracked CF-S39-H3-REJECT-LOG-SPAM (`audit/perf/s39-report.md:142`, a WARN per
   rejected H3 request) is ALREADY-KNOWN as *noise*; what is not recorded is that
   each of those lines is a synchronous, mutex-serialized syscall on a data-path
   worker, so the spam is also a throughput regression under a hostile client.

`observability.md` documents JSON-by-default and `LB_LOG_FORMAT`, but neither it
nor RUNBOOK states the writer is blocking or that no sampling exists.

**Existing test coverage: no.** `crates/lb-observability/tests/log_format.rs`
covers format selection only.

### REL-08 — the runtime label-cardinality guard is dead code

`crates/lb-observability/src/label_budget.rs:1-4` states the design:

```rust
//! RED metrics label budget: `prometheus` registers unbounded label sets happily, and one stray
//! `{path = <request URI>}` saturates a scrape with millions of series. Two gates for two label
//! kinds — [`LabelBudget::check`] is a startup worst-case product check for CLOSED sets, and
//! [`EnforcedLabelBudget`] is the per-emission guard for OPEN sets driven by request data.
```

`EnforcedLabelBudget` (`:154`) and its `admit()` (`:230`) appear in exactly three
places in the repository: its own definition, its own `#[cfg(test)]` module
(`:331,341,364`), and the `pub use` re-export in `lib.rs:43`. **No production
call site exists.** Only the startup product check is wired
(`crates/lb/src/main.rs:2138-2157`), and that multiplies *config-derived* counts
— it cannot bound a label whose value comes from request data.

Today this is latent rather than exploitable, because the one open-set label at
the emit site is hard-coded empty (`route_label = ""`, `main.rs:3247`). The
defect is that the guard designed for the moment `route` becomes real is not
attached to anything, and the two comments that assert protection are already
false:

- `main.rs:1897-1898`: *"REL-2-08: the label set is bounded on purpose — `route`
  is capped by MAX_ROUTES_BUDGET so a hostile path cannot explode the series
  count."* Nothing caps it; nothing consults `MAX_ROUTES_BUDGET` at emit time.

Additionally, **there is no metric or log for the budget being hit.** `admit()`
returns `CardinalityErr::Refused` and the doc comment on `observed()` (`:247`)
promises a `metrics_series_observed_total{family}` family that is registered
nowhere. When the guard is eventually wired, a refused tuple will silently drop
the request from `http_requests_total` with no counter-part signal — the correct
choice (drop, not an `"other"` bucket) executed invisibly.

Other open-set labels already in flight and outside any budget:
`backend_health_status{listener,backend}` (`main.rs:2012-2018`) and
`cert_loaded_at_seconds{listener}` / `lb_drain_timeout_ms_listener{listener}`.

**Existing test coverage: no.** `crates/lb-observability/tests/red_label_budget.rs`
tests only `LabelBudget::check` and the `CANONICAL_LABELS` table; it never
asserts that any emit site calls `admit`.

### REL-09 — `/metrics` exposition runs on the data-path runtime, unbounded

`crates/lb-observability/src/admin_http.rs:73-81`:

```rust
    match request.uri().path() {
        "/metrics" => {
            let body = render_text(registry);
```

`render_text` is fully synchronous: `registry.gather()` clones every metric
family into protobuf structures, then `TextEncoder::encode` formats the whole
body — no `.await`, no `spawn_blocking`
(`crates/lb-observability/src/prometheus_exposition.rs:12-25`). At the documented
budget ceiling (10 000 series, `DEFAULT_MAX_LABEL_CARDINALITY`) this is tens of
milliseconds of uninterruptible CPU on a tokio worker.

The admin listener spawns onto the *same* runtime as the data path
(`admin_http.rs:183` `tokio::spawn`, called from `async_main` on the single
`new_multi_thread` runtime at `main.rs:2109`), and there is **no concurrency
limit**: every accepted connection gets its own `tokio::spawn`
(`admin_http.rs:200-210`) with no semaphore, no queue, and no per-request
timeout. *k* concurrent scrapers ⇒ *k* workers doing a full gather+encode
simultaneously. When *k* ≥ worker count the data path stops.

`docs/guide/observability.md` ("Scrape configuration", 15 s interval) and
`METRICS.md` give a scrape config but never state that exposition is on the
serving runtime or that concurrent scrapes are unbounded.

Related, same file: `hyper::server::conn::http1::Builder::new().keep_alive(true)`
(`admin_http.rs:203-204`) sets no `header_read_timeout` and there is no
connection cap, so a slowloris or a stuck scraper pins fds out of the *process*
budget the data path shares (REL-35).

**Existing test coverage: no.** `tests/metrics_endpoint.rs` has three tests, all
sequential single-request GETs against a standalone listener.

### REL-10 — `http_request_duration_seconds` is a connection-lifetime histogram with 10 s buckets

`crates/lb/src/main.rs:3084` starts the clock at connection accept:

```rust
            let http_start = Instant::now();
```

and `main.rs:3257-3264` observes it after `serve_connection` returns:

```rust
                if let Ok(h) = st.metrics.histogram_vec(
                    "http_request_duration_seconds",
                    "L7 request duration from accept to response body sent",
                    &["listener", "route", "version"],
                    &http_latency_buckets(),
                ) {
                    h.with_label_values(&[listener_label, route_label, version])
                        .observe(http_start.elapsed().as_secs_f64());
                }
```

`http_latency_buckets()` (`crates/lb-observability/src/lib.rs:403-409`) tops out
at `10.0`.

The per-connection *scope* is ALREADY-KNOWN (`METRICS.md:55,76`; `RUNBOOK.md`
LbReqDuration "per-connection histogram"). The consequence is not recorded: an
H1/H2 keep-alive connection lives for the idle timeout (tens of seconds) or the
`HttpTimeouts::total` bound (60 s per the CI notes), so **essentially every
observation lands in the `+Inf` bucket**. `histogram_quantile` then reports the
highest finite boundary, a constant `10.0`, and the documented alert
`histogram_quantile(0.99, ...) > 1.0` (`RUNBOOK.md:483`, and one of the four
starter alerts in `observability.md:120-127`) is **permanently true** on any
listener with keep-alive enabled. An always-firing page-adjacent alert is an
alert that gets silenced, taking the real latency signal with it.

The help string — `"L7 request duration from accept to response body sent"` —
also reads as per-request to anyone building a dashboard from `/metrics` alone.

**Existing test coverage: no.** `tests/metrics_label_shape.rs:102-103` observes a
hand-picked `0.1` and asserts label keys only.

---

## MEDIUM

### REL-11 — the G5 passive-health family is undocumented and unalerted

`crates/lb/src/main.rs:1988-2018` registers, and the 5 s pump at `:2023-2101`
publishes:

- `backend_ejections_total{listener}`
- `backend_readmissions_total{listener}`
- `backend_ejections_suppressed_total{listener}` — help text:
  `"G5: ejections REFUSED by the minimum-healthy floor (failing backend left in rotation)"`
- `backends_ejected{listener}`
- `backend_health_status{listener, backend}` — `0=unknown, 1=healthy, 2=unhealthy`

None of these five names appears in `docs/guide/METRICS.md` or
`docs/guide/RUNBOOK.md` (grep confirms zero hits in both). Passive health
ejection is the newest shipped feature (S46, `01915a77`).

The code already knows this is page-grade. `crates/lb/src/main.rs:1977-1978`:

```rust
/// `backend_ejections_suppressed_total` is the one an operator must page on: it means a backend is
/// failing AND only the minimum-healthy floor is keeping the listener serving.
```

That decision never reached the operational docs. There is no alert, no runbook
entry, and no dashboard guidance for a condition the implementers explicitly
designated as page-worthy — the gateway has positively identified a failing
backend and is *still* routing to it. RUNBOOK claims to be "the single source of
truth for **every alert that can fire** against the metrics in `METRICS.md`";
these five metrics are in neither document, so the claim holds only vacuously.
This is a doc/alert-catalog gap, not a design gap: the fix is a RUNBOOK alert
entry plus METRICS.md rows, not new code.

The pump itself is correct: it holds `last: HashMap<listener, (u64,u64,u64)>`
and does `if cur > was { inc_by(cur - was) }` (`main.rs:2095`), so
the counters are monotonic across pump ticks. No defect there.

### REL-12 — the soak DRIFT test is conjunctive; a jittery leak escapes

`crates/lb-soak/src/timeseries.rs:334-336`:

```rust
        MetricKind::Trend => {
            if rel_growth > cfg.band && mono >= cfg.monotone_min {
```

with `band = 0.10` and `monotone_min = 0.60` (`:51-54`). `mono` is the fraction
of consecutive sample-to-sample deltas that are non-negative (`:207-225`).

**Scenario.** A 1 MB/min leak on a 500 MB RSS baseline over a 4 h soak is +48 %
growth — unambiguous. Sampled every 15 s that is +250 KB per sample, against
allocator/arena jitter of several MB per sample. Most consecutive deltas are
therefore dominated by noise and `mono ≈ 0.5 < 0.60` ⇒ **BOUNDED**. The verdict
note even prints the escape hatch as if it were reassurance:

```rust
                        "{column}: bounded — last-third median {l3} vs first-third {f3} \
                         ({:+.1}%, within band {:.0}% or non-monotone {:.0}%)",
```

i.e. it will happily print "+48.0 %, ... non-monotone 51 %" and call it BOUNDED.

The statistic that *is* robust to per-sample jitter — the least-squares slope
over the trimmed window — is computed on the very next line (`slp = slope(trimmed)`,
`:307`) and then used only for display. The right instrument is built and
discarded.

Note the known false-*positive* modes (low-baseline sawtooth, t=0 boot outlier)
are ALREADY-KNOWN and are documented in-code at `eg-soak.rs:923-931`. This is the
opposite direction — a false *negative* — and is not recorded anywhere.

### REL-13 — a must-be-zero counter with zero samples reports "stayed zero across the run"

`crates/lb-soak/src/timeseries.rs:236-260`:

```rust
    if kind == MetricKind::CounterMustBeZero {
        let v = if max > 0.0 { Verdict::Drift } else { Verdict::Bounded };
        let note = if max > 0.0 {
            format!("{column}: must-be-zero counter reached {max}")
        } else {
            format!("{column}: stayed zero across the run")
        };
```

`max` is initialised from the NaN-filtered sample vector and forced to `0.0`
when there are no finite samples (`:234`: `let (min, max) = if n == 0 { (0.0, 0.0) } else { (min, max) };`).
This branch runs **before** the `tn < cfg.min_samples ⇒ Inconclusive` guard
(`:275`), so it has no minimum-sample protection at all.

`panic_total` is `CounterMustBeZero` in every scenario (`eg-soak.rs:920-922,
933-935, 940-942, 951-957, 966-971`). If the metrics scrape failed for the entire
run (REL-15), the column is all-NaN, `n == 0`, and the analyzer emits the literal
string **"panic_total: stayed zero across the run"** having observed nothing. An
operator or reviewer reading `summary.txt` is told a fact the harness never
measured.

### REL-14 — the connection-leak discriminant reads zero when it cannot read

`crates/lb-soak/src/procstat.rs:36-41`:

```rust
pub fn count_fds(proc_dir: &Path) -> u64 {
    let fd_dir = proc_dir.join("fd");
    match std::fs::read_dir(&fd_dir) {
        Ok(entries) => entries.count() as u64,
        Err(_) => 0,
    }
}
```

`crates/lb-soak/src/bin/eg-soak.rs:923-926` names `fds` as the leak proof:

```rust
/// sc8_ws_h1 connection-leak DISCRIMINANT: the OS file-descriptor count (`fds`). Every live WS
/// tunnel pins a client fd + a backend fd + the relay task, so a bounded series IS the no-leak
/// proof.
```

`/proc/<pid>/status` is world-readable, so `sample_pid` returns `Some(..)`
whenever the process exists; `/proc/<pid>/fd` requires matching uid (or
`CAP_SYS_PTRACE`). Run the soak as a different user from the gateway — or in a
different PID namespace, i.e. the containerised case — and `fds` is a flat `0`
for the whole run. A perfectly flat series is BOUNDED, so scenarios `sc7`, `sc8`,
`sc8b`, `sc8c` (whose only leak signal is `fds` + RSS, per `h3term_gauges` /
`ws_gauges` at `eg-soak.rs:933-942`) pass with the discriminant switched off and
no indication in the output.

`Err(_) => 0` is the wrong absorbing value: `0` is a legal reading. The doc
comment at `procstat.rs:42-44` records the behaviour ("an unreadable fd dir falls
back to 0") without noting that it silently invalidates the verdict.

### REL-15 — every scrape failure becomes an empty metric set

`crates/lb-soak/src/sampler.rs:35-40`:

```rust
pub async fn scrape(metrics_addr: SocketAddr) -> MetricSet {
    match http_get(metrics_addr, "/metrics").await {
        Ok((200..=299, body)) => metrics::parse(&body),
        _ => MetricSet::default(),
    }
}
```

The `_` arm swallows: connect timeout (3 s, `:19`), read timeout (5 s, `:23`),
**403 from the admin bearer gate**, any non-2xx, and any transport error. There
is no failed-scrape counter and no log. `build_row` then converts the empty set
to `NaN` per gauge (`:88`), and `analyze_column` filters NaN out (`:231`), so
every gauge column silently loses samples.

Chain with REL-13: a run whose `/metrics` was unreachable throughout reports
`panic_total: stayed zero across the run` (BOUNDED), all other gauges
`Inconclusive` (not DRIFT), `overall_drift == false` ⇒ marker `overall=BOUNDED`
⇒ `soak-verdict.sh` **PASS**. The harness cannot distinguish "the gateway was
healthy" from "we never measured the gateway."

The unit tests encode the behaviour as intended (`sampler.rs:121-129`,
`scrape_dead_port_is_empty_set`), which is why it will not be caught by review of
the tests alone.

### REL-16 — the cardinality warning is blind to the explosion it guards against

`crates/lb-observability/src/lib.rs:386-400`:

```rust
    fn check_cardinality(&self) {
        let total: usize = self
            .inner
            .gather()
            .iter()
            .map(|fam| fam.get_metric().len())
            .sum();
        if total > CARDINALITY_WARN_THRESHOLD {
            tracing::warn!(...);
        }
    }
```

`check_cardinality()` is called **only** from the `Entry::Vacant` arm of each
getter (`lib.rs:138, 170, 204, 240, 272, 299`) — i.e. once per new metric
*family*. Cardinality does not explode by adding families; it explodes by adding
label *values* to an existing family. Once the ~20 families are registered at
boot, this function never runs again, and a `CounterVec` that grows to a million
series triggers nothing.

Secondary cost: the check calls `self.inner.gather()`, which materialises every
metric family into protobuf, on each registration. Harmless at boot; a latent
O(series) cost if any code path ever registers a family lazily under load
(`increment(name, value)` at `lib.rs:355-362` creates counters on first touch, so
a dynamic name would do exactly that).

### REL-17 — H3 has no trace context, and nothing emits child spans

`RequestTrace::open` is called from exactly two sites — `crates/lb-l7/src/h1_proxy.rs:571`
and `crates/lb-l7/src/h2_proxy.rs:644`. Grep for `traceparent|tracestate|RequestTrace`
across `crates/lb-quic/src/` returns **nothing**. Consequences:

- An H3 client's `traceparent` is neither consumed nor replaced. Whatever the
  header pass-through does, the gateway contributes no span, so the trace has a
  hole at the hop where latency is being introduced.
- The trace-context work is a documented feature (ROUND8-OPS-06 / REL-2-07,
  `trace_ctx.rs:1-2`) with no stated protocol scope, so an operator reasonably
  expects it on all three protocols.

Separately, `tracing::info_span!("lb.l7.request", ...)` at `trace_ctx.rs:152` is
the **only** span in the workspace — grep for `info_span!|debug_span!|span!(`
returns that one construction site plus two `.instrument()` calls
(`h1_proxy.rs:584`, `h2_proxy.rs:659`). There is no child span for backend
selection and none for upstream timing. A trace therefore shows total
gateway-side duration and nothing that decomposes it. This compounds with the
documented-pending `backend_request_duration_seconds`: RUNBOOK's `LbReqDuration`
diagnose step 2 already tells the operator to fall back to the backend's own
metrics because the per-backend split is pending — and the trace cannot answer
the question either.

`http.status_code` *is* recorded (`h1_proxy.rs:586`, `h2_proxy.rs:661`), so the
span is not vacuous.

### REL-18 — concurrent root requests can be minted with identical trace-ids

`crates/lb-l7/src/trace_ctx.rs:140-146`:

```rust
        let span_seed = SPAN_COUNTER.load(Ordering::Relaxed);
        let (trace_id, inbound_parent, flags) = match parsed {
            Some(ctx) => (ctx.trace_id, Some(ctx.parent_id), ctx.flags),
            // Root: sample bit on so the span exports.
            None => (synth_trace_id(span_seed), None, 0x01),
        };
        let span_id = mint_span_id();
```

The seed is a plain `load`, not the value returned by the `fetch_add` inside
`mint_span_id()` (`:77`). Two threads entering `open()` in the same window read
the same counter value; `synth_trace_id` is a pure function of
`(startup_nonce, seed)`, so both mint the **same trace-id**. The span-ids differ
(the `fetch_add` is atomic), so the collision is silent.

This affects only the root branch — requests arriving without a `traceparent`,
which is all edge traffic. In a backend trace store the trace-id is the primary
key: two unrelated requests merge into one trace with two disjoint span trees.
On a load balancer the window is exercised continuously.

Fix shape (for the triage pass, not applied here): seed from the `fetch_add`
result rather than a separate `load`.

**Existing test coverage: no.** `trace_ctx.rs:256-265`
(`span_ids_are_unique_per_call`) asserts distinct `parent_id`s from two
*sequential* calls, which is exactly the case that already works.

### REL-19 — the admin accept loop is silent and spins under fd exhaustion

`crates/lb-observability/src/admin_http.rs:193-199`:

```rust
            let (stream, peer) = match accepted {
                Ok(v) => v,
                Err(e) => {
                    tracing::debug!(error = %e, "admin accept error");
                    continue;
                }
            };
```

Three problems in six lines, all of which bite at the same moment:

1. `tracing::debug!` — invisible at the default `info` level. RUNBOOK's default
   is `info` ("Default log level is `info`").
2. No backoff. `accept()` under `EMFILE`/`ENFILE` returns immediately, so the
   loop spins at 100 % of a core. This is precisely the case the data-path accept
   loop handles ("The handler implements exponential backoff (start 1 ms, cap 1 s)
   on persistent error kinds, so the CPU should not spin", `RUNBOOK.md:399-401`);
   the admin loop has no equivalent.
3. No `accept_errors_total{listener,kind}` bump, so `LbAcceptErrors` — the
   **page**-severity EMFILE alert — cannot fire for the admin listener.

Net effect: during an fd-exhaustion incident the metrics/probe endpoint burns a
core, stops answering, logs nothing at the configured level, and moves no
counter. The instrument fails exactly when it is needed and reports its own
failure nowhere.

The per-connection arm has the same level problem, plus the S46 error-chain issue:
`admin_http.rs:208` logs `error = %e` on a `hyper::Error`, whose top-level
`Display` is famously uninformative and whose `source()` is not walked.

### REL-20 — H3 abort taxonomy loses the cause inside each variant

The variants *are* surfaced — `crates/lb-quic/src/conn_actor.rs:1035,1053,1070`
log `tracing::warn!(?abort, stream_id = sid, "H3→Hx resp stream aborted")`. What
does not survive is what caused them.

`crates/lb-quic/src/h3_bridge.rs:944-952`:

```rust
    while let Some(frame_res) = body.frame().await {
        let frame = match frame_res {
            Ok(f) => f,
            Err(_) => {
                // Upstream body error mid-response ⇒ Reset, never a clean FIN.
                let _ = tx.send(RespEvent::Reset).await;
                return Err(RespAbort::UpstreamReset);
            }
        };
```

`frame_res` is a `hyper::Error`. It is discarded by `Err(_)` and never logged, so
the operator gets `abort: UpstreamReset` and nothing else — the exact shape of
the CF-S44 defect S46 spent two months on.

Two variants also fold operationally *opposite* causes into one label
(`h3_bridge.rs:125-138`):

```rust
    /// `Transfer-Encoding: chunked` decode error, or EOF before the terminator.
    ChunkedDecode,
    /// HEADERS parse failure, or head over the head cap before `CRLF CRLF`.
    BadHead,
```

`BadHead` is returned both for a malformed status line (`:670,689,726` — fix the
backend) and for a response head exceeding the 64 KiB cap (`:659,713` — raise the
cap or shrink the headers). `ChunkedDecode` covers sixteen distinct return sites
(`:535,546,555,560,563,568,571,574,595,602,611,612,614,617,621,775`), spanning bad chunk hex,
non-UTF-8 in a chunk header, a missing colon in a trailer, and an oversize
trailer *section cap*. One log line; opposite remediations.

### REL-21 — `xdp_sampler_errors_total` has no `kind` label, but two sources say it does

`crates/lb-observability/src/xdp_metrics.rs:38-41`:

```rust
        let sampler_errors_total = registry.counter(
            "xdp_sampler_errors_total",
            "Times the XDP stats sampler failed to read the STATS map.",
        )?;
```

`registry.counter(..)` — an unlabelled `IntCounter`. Both of these claim otherwise:

- `crates/lb-observability/src/label_budget.rs:40`:
  `("xdp_sampler_errors_total", &["kind"]),`
- `docs/guide/RUNBOOK.md:441-444` (`LbXdpSamplerErrors`, **Wired: yes**):
  "`kind` label distinguishes `map_lookup` / `permission` / `other`. **Diagnose**:
  usually a permissions issue (`CAP_BPF` missing) …"

The runbook's entire diagnostic step is "read the `kind` label"; there is no such
label. `CAP_BPF`-missing and a map-name mismatch produce an identical, bare
counter increment.

The reason this drift survived: `label_budget.rs:16-17` claims
*"`tests/red_label_budget.rs` diffs the live registry against it, so anything
unlisted is a regression"* — that test does no such diff. It checks that names
appear in the table (`red_label_budget.rs:27-39`). `tests/metrics_label_shape.rs`
hand-checks two families. **Nothing compares `CANONICAL_LABELS` to the registry.**

### REL-22 — `shutdown_aborted_connections_total` is unlabelled; its alert groups by `listener`

`crates/lb/src/main.rs:2670-2676` and `:3225-3231` both register it via
`metrics.counter(...)` — a plain, unlabelled counter:

```rust
        if let Ok(c) = metrics.counter(
            "shutdown_aborted_connections_total",
            "Tasks still live when the drain deadline elapsed",
        ) {
            c.inc_by(report.in_flight_remaining as u64);
        }
```

`RUNBOOK.md` `LbShutdownTruncatedStreams` specifies:

```promql
sum by (listener) (rate(shutdown_aborted_connections_total[1h])) > 0
```

and diagnose step 1 is "Confirm the listener carries streaming traffic." The
label does not exist, so the query collapses to one series and per-listener
attribution — the entire point of the alert — is impossible. RUNBOOK partially
hedges ("Wired: partial … the streaming-listener correlation is operator
judgement") but still publishes PromQL that implies a label.

Also note the two registration sites use **different help strings** for the same
metric name ("Per-connection tasks cancelled mid-flight by SIGTERM drain" vs
"Tasks still live when the drain deadline elapsed"). `MetricsRegistry::counter`
documents that on a cache hit `help` is ignored (`lib.rs:116`), so whichever site
runs first wins; harmless, but the exposed `# HELP` is order-dependent.

### REL-23 — `init_tracing` reports "already initialised" for any failure

`crates/lb-observability/src/log.rs:88-94`:

```rust
    match install_result {
        Ok(()) => { let _ = INIT.set(format); Ok(()) }
        Err(_) => Err(TracingError::AlreadyInitialised),
    }
```

`TryInitError` is discarded and every cause is renamed to one that may be false.
`try_init` fails both when a global default subscriber exists *and* when the
`log` compatibility tracer cannot be installed (another `log` logger already
registered). `TracingError` has a single variant with no `#[source]`, so the
chain is severed at the type level.

At the call site (`crates/lb/src/main.rs:2118-2120`):

```rust
    match lb_observability::init_tracing(&lb_observability::TracingConfig::default()) {
        Ok(()) | Err(lb_observability::TracingError::AlreadyInitialised) => {}
    }
```

both arms are no-ops, so a genuine failure to install the subscriber means the
process runs to completion **with no logs at all** and never says why. This is
the production form of the S46 lesson ("tracing output needed a subscriber that
tests did not install").

### REL-24 — an H3→H1 request-write failure produces a 502 with no log

`crates/lb-quic/src/h3_bridge.rs:1240-1244`:

```rust
            Err(()) => {
                inline(&resp_tx, 502, b"bad gateway").await;
                pooled.set_reusable(false);
                return Ok(());
            }
```

`write_h1_request` returns `Result<_, ()>` — the unit error type, so the cause is
destroyed at the signature. The arm returns `Ok(())`, so `conn_actor`'s
`if let Err(abort)` warn (`conn_actor.rs:1068-1071`) never fires. The client gets
a 502 and there is no log line, no counter, and no trace event anywhere. The
`ReqWriteOutcome::Aborted(status, body)` arm immediately above (`:1234-1239`) is
equally silent.

Contrast the sibling arm at `:1213-1219`, which does it right:

```rust
        Err(e) => {
            tracing::warn!(error = %e, "H3→H1 resp stream backend acquire failed");
```

---

## LOW / INFO

### REL-25 — probe body token is `booting`; RUNBOOK documents `starting`

`crates/lb-observability/src/probes.rs:36-42`:

```rust
    pub const fn body_token(self) -> &'static str {
        match self {
            Self::Starting => "booting",
```

`RUNBOOK.md` (§ Health probe endpoints): *"returns the same JSON body,
`{"status":"<state>"}\n` (`<state>` is one of `ok` / `starting` / `draining`)"*.
An operator who follows RUNBOOK and greps for `"status":"starting"` during a slow
boot matches nothing. `probes.rs:163-167` and
`crates/lb-observability/tests/health_endpoints.rs:56` both pin `"booting"`, so
the code is self-consistent and the doc is wrong — the fix belongs in the doc.

### REL-26 — `.ok()?` silently disables whole metric families

`crates/lb/src/main.rs:177-206` (`CertMetrics::register`) and `:218-262`
(`ReloadMetrics::register`) chain `.ok()?` on every registration, so a single
failure returns `None` for the entire struct. Call sites (`:2407`, `:2409`) bind
the `Option` and never log the `None`:

```rust
    let cert_metrics = CertMetrics::register(&metrics);
```

Downstream, `reload_all_tls` and `reload_config` take `Option<&…Metrics>` and
skip every bump. `LbCertRotationFailed` is a **page**-severity alert; it would be
silently disarmed with no diagnostic. Registration failure is unlikely (it needs
a name/type collision), which is why this is LOW rather than higher — but it is a
`.ok()` on a Result that mattered.

### REL-27 — `tracestate` entry limit is not enforced; oversize is dropped silently

`crates/lb-observability/src/tracing_propagation.rs:171-173`:

```rust
    let tracestate_raw = headers
        .get_first(TRACESTATE_HEADER)
        .filter(|v| v.len() <= TRACESTATE_MAX_LEN);
```

Only the 512-byte bound is applied. W3C Trace Context §3.3.1.1 also caps
`tracestate` at **32 list-members**; a 512-byte value can carry >100 tiny members
(`a=1,b=2,…`), which the gateway forwards verbatim to the backend, where a
spec-conformant collector will reject or truncate the whole header.

The drop is also silent: no log, no metric. An operator debugging "our vendor's
baggage disappears at the gateway" has no signal to work from. `parse_traceparent`
itself is correct and rigorous — length, separator positions, `ff` version
rejection, and all-zero trace-id/parent-id rejection are all implemented
(`:97-140`) — so this is a narrow gap in an otherwise solid codec.

### REL-28 — a malformed or future-version `traceparent` severs the trace, indistinguishably

`crates/lb-observability/src/tracing_propagation.rs:167-168` provides the raw
bytes precisely so a non-parsing header can be forwarded:

```rust
/// Extract the inbound context, returning the raw bytes so an unparseable header still forwards.
```

The sole consumer discards it (`crates/lb-l7/src/trace_ctx.rs:133-138`):

```rust
        let ExtractedContext {
            parsed,
            tracestate_raw,
            ..
        } = tracing_propagation::extract_parent(&bag);
```

so `traceparent_raw` is dead, and `parse_traceparent` returning `None` collapses
**three different situations** into one behaviour — mint a new root trace:

| Cause | Correct behaviour | Actual |
|---|---|---|
| No `traceparent` at all | new root | new root ✓ |
| Malformed `traceparent` (buggy client) | new root, but *observably* | new root, silent |
| Version `01+` (W3C §3.2.2.3 says parse the first 3 fields and continue) | continue the trace | trace severed |

Nothing logs and nothing counts, so the operator cannot tell "clients are not
instrumented" from "clients are sending garbage" from "a newer W3C version is in
the fleet." This is the same shape as the S46 "one arm serving two opposite
triggers" defect. (Accepting an untrusted inbound `traceparent` from the internet
is a reasonable default, but it is an undocumented decision — worth one line in
`observability.md`.)

### REL-29 — the release binary cannot produce the backtrace the runbook asks for

`crates/lb/src/main.rs:75` captures unconditionally:

```rust
        let bt = Backtrace::force_capture();
```

The release profile sets `strip = "symbols"` (root `Cargo.toml`) and
`docker/Dockerfile:33` runs `strip target/release/expressgateway` a second time.
The captured frames therefore render without symbol names. `RUNBOOK.md:288-297`
directs the on-call to "find the structured line with `location=<file:line>` and
`backtrace=<...>`" and to "Capture the panic stack trace … file an issue." The
`location` field works (it comes from `panic::Location`, resolved at compile
time); the `backtrace` field does not. Consider `debug = 1` / `split-debuginfo`
with a symbol server, or drop the field and say so.

Secondary: `backtrace = %bt` is a multi-line value. In JSON mode it is escaped
into one object (fine); with `LB_LOG_FORMAT=text` it breaks the one-event-per-line
contract that log shippers rely on.

### REL-30 — the systemd unit cites two CI gates that do not exist

`packaging/expressgateway.service:4-24` claims both:

- *"the doc-lint job enforces that every directive named in `DEPLOYMENT.md`
  appears here"* — `scripts/ci/doc-lint.sh` contains exactly one systemd rule
  (`:51`, a negative lint on `ExecStart=/usr/local/bin/lb`).
- *"CI gate `systemd-analyze-security` enforces"* — no such job exists in
  `.github/workflows/{ci,scheduled,release}.yml`.

These two absent gates are why REL-02 and REL-05 (both pure unit-vs-doc
divergences) were able to ship.

### REL-31 — `backend_health_status` series are never removed

`crates/lb/src/main.rs:2012-2018` registers a `GaugeVec` keyed
`{listener, backend}`, and the 5 s pump `set`s a value per live backend. Nothing
calls `remove_label_values`. After a SIGHUP that changes the backend pool, the
retired backend's series persists at its last value — typically `1` (healthy) —
forever. A dashboard or an alert on "healthy backends per listener" over-counts,
and the count only ever grows across reloads. Same class as a leaked gauge,
triggered by a config change rather than an error path.

### REL-32 — `eg-soak` exits 0 on DRIFT

`crates/lb-soak/src/bin/eg-soak.rs:167-208` computes `overall_drift`, writes the
marker, prints `DRIFT(finding)`, and returns `Ok(())`. The only non-zero exits
are argument errors (`:75`, `:233`). The release path is safe —
`scripts/soak/soak-verdict.sh` reads `overall=BOUNDED` from the marker — but any
direct invocation (e.g. `scripts/archive/s39-burnin.sh`, or a future CI step that
checks `$?`) reads a green exit code from a detected leak.

### REL-33 — RUNBOOK diagnose steps reference metrics registered nowhere

`RUNBOOK.md` `LbXdpConntrackFull` step 1: *"`xdp_conntrack_entries_current{family}`
vs `xdp_conntrack_capacity{family}` — confirm the map is at limit."* Neither name
is registered anywhere (`xdp_metrics.rs::register` creates
`xdp_packets_total`, `xdp_conntrack_full_total`, `xdp_sampler_errors_total`,
`xdp_attached_mode` only). `xdp_bytes_total` is likewise declared in
`CANONICAL_LABELS:38` and never registered.

The *alerts* are correctly flagged `Wired: pending` — that part is
ALREADY-KNOWN (`METRICS.md:280-282`). The gap is that the **Diagnose** steps
carry no pending marker, so an on-call following the triage matrix runs a `curl |
grep` that returns nothing and has to work out whether that means "healthy" or
"not implemented."

### REL-34 — `quic_modeb_streams_active` is last-writer-wins

`crates/lb-observability/src/quic_modeb_metrics.rs:18-20` documents it honestly
in code:

```rust
    /// Relay-stream table size after each pass. Under concurrent actors this reflects only the
    /// MOST RECENT actor's table, not a sum — it is a bounded-state signal, not a total.
    pub streams_active: IntGauge,
```

The caveat does not appear in `METRICS.md`, and the metric *is* used as a soak
gauge (`eg-soak.rs:945-950`) under concurrent Mode-B load, where it oscillates
with whichever actor wrote last. As a Trend column this feeds the analyzer noise
that (per REL-12) pushes `monotone_frac` down and biases toward BOUNDED.

### REL-35 — no admin connection cap or header-read timeout

`crates/lb-observability/src/admin_http.rs:201-210` spawns a task per accepted
connection with `http1::Builder::new().keep_alive(true)` and no
`header_read_timeout`, no idle timeout, and no connection cap. A stuck scraper or
a local slowloris holds fds out of the process-wide budget the data path shares,
and there is no metric for admin connection count. Loopback-only binding keeps
the blast radius local (`AdminAuthGate::validate_bind`, `main.rs:2275-2280`),
hence LOW.

---

## Checked and found sound (no finding)

Recorded so the next reviewer does not re-derive them:

- **No deadline is computed from `SystemTime`.** Every wall-clock use is a
  nonce/seed or a display timestamp: `shutdown.rs:543` (jitter seed),
  `trace_ctx.rs:60` (startup nonce), `ticket.rs:427` (`loaded_at_wall`, display
  only — `rotate_if_due` takes `Instant`, `ticket.rs:118`), `main.rs:281`
  (`cert_loaded_at_seconds` gauge), and per-crate port randomisation. An NTP step
  cannot break a timeout.
- **`AcceptInflightGuard` is leak-free.** `main.rs:692-708` is RAII; the guard is
  constructed before the spawn and moved into the future, so it decrements on
  every path including a dropped (never-polled) future.
- **The G5 health pump is monotonic-safe** — `if cur > was { inc_by(cur - was) }`
  keyed per listener; no counter can decrease.
- **`MetricsDrainObserver::BUCKETS`** = `[0.1 … 300.0]` (`main.rs:2857`) does
  cover the documented 300 000 ms maximum per-listener drain budget, and the
  `LbShutdownSlow` threshold (0.8 × budget) sits below the top finite bucket. The
  buckets are right; only their scrapability is not (REL-01).
- **Bounded queues everywhere on the data path** — `H2_REQ_CHANNEL_DEPTH`,
  `H3_BODY_CHANNEL_DEPTH`, `H3_RESP_CHANNEL_DEPTH`; the full policies are block
  (HTTP bodies, via `send().await`), drop-newest (QUIC DATAGRAM, counted by
  `quic_modeb_datagrams_dropped_total`), and shed-at-accept (`accept_shed_total`,
  `LbAcceptShed`). They are *implemented*; they are not collected in one
  documented table, which is a doc gap rather than a defect.
- **`parse_traceparent`** correctly rejects short/long input, misplaced
  separators, version `ff`, non-`00` versions, and all-zero trace-id and
  parent-id (`tracing_propagation.rs:97-140`).
- **`reload_all_tls`** is a model error arm: it logs both the low-cardinality
  `reason` label *and* `error = %e`, and keeps the previous bundle live
  (`main.rs:322-333`).
- **`docker/Dockerfile`** is distroless `cc-debian12:nonroot`, explicit
  `USER 65532:65532`, exec-form ENTRYPOINT, OCI provenance labels, no shell. The
  absent `HEALTHCHECK` is a documented Phase-1 decision (`:71-76`).
- **Log injection is contained in the default configuration.** JSON is the
  default formatter and `tracing-subscriber`'s JSON layer escapes field values
  via `serde_json`. `json_status` (`admin_http.rs:120-131`) hand-formats JSON but
  only over the closed `body_token` set, and says so. Under
  `LB_LOG_FORMAT=text` values are unescaped, but the CR/LF needed to forge a line
  cannot reach a logged field: hyper/h2/h3 reject CR/LF in header values and
  request targets, and rustls validates SNI as a DNS name.

---

## Notes on artifacts

- `RUNBOOK.md` already exists at `docs/guide/RUNBOOK.md` and is thorough; the
  defects above are corrections to it (REL-04, REL-05, REL-21, REL-22, REL-25,
  REL-33) plus one addition (REL-11, the G5 family), not a rewrite. No new
  runbook was created — per the lead's instruction this pass is discovery only.
- Nothing in this report was verified by execution; no cargo command was run.
  Each claim is traceable to the quoted line. The claims most worth re-checking
  under CI before a fix lands are REL-02 (a live `systemd` start under
  `Type=notify`) and REL-18 (a concurrency stress on `RequestTrace::open`).
