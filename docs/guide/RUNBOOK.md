# Runbook

Operational procedures for ExpressGateway. Assume `systemd` is the
process supervisor (see `DEPLOYMENT.md`) and Prometheus scrapes the
admin listener on `127.0.0.1:9090`.

This file is the single source of truth for **every alert that can
fire** against the metrics in `METRICS.md`. Cross-references in the
form `REL-2-NN` / `SEC-2-NN` point at the audit files under
`audit/reliability/` and `audit/security/`.

---

## Startup

1. `systemctl start expressgateway`.
2. `systemctl status expressgateway`.
3. Look for these log lines (order matters — a missing line signals a
   config or environment problem):
   - `ExpressGateway v<version>`
   - `configuration loaded from <path>` with `listeners=N`
   - `lb-io runtime ready backend=io_uring|epoll high_water=65536 low_water=32768`
   - `TCP backend pool ready`
   - `DNS resolver ready`
   - one `listener ...` line per entry in `[[listeners]]`
   - `metrics endpoint listening on <addr>` (when `[observability].metrics_bind` is set)
   - `panic hook installed; panic_total counter bound`

If you see `listener has no backends configured — skipping`, the
listener was dropped; inspect the TOML.

## Drain (graceful shutdown)

```
systemctl stop expressgateway
```

Sends SIGTERM. The binary's lifecycle handler in `crates/lb/src/main.rs`
performs the sequence below in order; each step is observable in the
log stream:

1. `lifecycle signal received signal=SIGTERM`.
2. `entering drain — flipping /readyz to 503` — `ProbeRegistry` marks
   `draining = true`. External LBs scraping `/readyz` see 503 on the
   next probe (typically ≤ 10 s) and stop sending new traffic.
3. `settling for upstream LB before cancel` — sleep for
   `[runtime].readiness_settle_ms` (default 11 000 ms — ROUND-8
   OPS-11) so the upstream probe round-trip lands. See "Tuning
   `readiness_settle_ms`" below for why the default is one full
   kubelet probe period plus margin.
4. `shutdown.token().cancel()` — cooperative `select!` arms in
   long-lived tasks observe the cancel.
5. Listener `JoinHandle::abort()` for every TCP accept loop.
6. `QUIC listeners did not drain within 2s` — at most. Listeners hold
   their own cancel tokens; H3 actors emit `CONNECTION_CLOSE
   error_code=0x0100` (H3_NO_ERROR, RFC 9114 §8.1) per PROTO-2-11.
7. `draining tasks deadline_ms=<N>` — bounded wait on
   `lb_core::Shutdown::drain(deadline)`. The deadline is
   `[runtime].drain_timeout_ms` (default 10 000 ms). The TaskTracker's
   live count is logged.
8. On overflow: `drain deadline elapsed — survivors will be aborted on
   runtime drop` and `shutdown_aborted_connections_total` is bumped by
   the remaining count.
9. `_xdp_loader` drops LAST so the userspace inserter sees a stable
   map until the very last handler has exited.
10. `ExpressGateway stopped` summary line with `total_connections`,
    `bytes_in`, `bytes_out`.

Unit-file knobs that matter:
- `KillMode=mixed` — main process gets SIGTERM, spawned children
  receive SIGKILL after `TimeoutStopSec`.
- `TimeoutStopSec=30` — should exceed `drain_timeout_ms` plus
  `readiness_settle_ms` plus the 2 s QUIC budget so systemd doesn't
  SIGKILL mid-drain. With the ROUND-8 OPS-11 defaults this is
  `11 s (settle) + 10 s (drain) + 2 s (QUIC) = 23 s`, leaving 7 s of
  headroom under the conventional `terminationGracePeriodSeconds:
  30` / `TimeoutStopSec=30`. If you raise either budget, raise the
  service-manager timeout to match.

### Tuning `readiness_settle_ms`

The lameduck window between flipping `/readyz` to 503 and starting
the cooperative cancel must be **at least one full upstream-probe
period plus margin** — otherwise the pod starts severing connections
while the upstream LB (kubelet, cloud LB, mesh sidecar) still lists
it Ready, and the next probe-period's worth of new connections land
on the draining pod.

K8s alignment: the kubelet default readiness-probe `periodSeconds`
is 10 s, and endpoint removal lags the pod `Terminating` transition
by up to one probe period (see the Kubernetes "Termination of Pods"
docs and Envoy's `drain_strategy` guidance, which both treat the
endpoint-removal lag — not the signal-handler latency — as the
quantity to wait out). The 11 000 ms default is `10 s + 1 s` so at
least one 503 lands inside the window even when `set_draining` fires
immediately after a probe.

| Upstream LB         | Typical probe period | Recommended `readiness_settle_ms` |
|---------------------|----------------------|-----------------------------------|
| kubelet (default)   | 10 s                 | 11000 (default)                   |
| kubelet, aggressive | 1 s                  | 1500                              |
| AWS ALB / NLB       | 30 s                 | 30000 (validation cap)            |
| GCP Load Balancer   | 5 s                  | 6000                              |

If you see traffic landing on a draining pod (`accept_inflight`
rising after `entering drain — flipping /readyz to 503`), the window
is too short for your upstream's probe period.

### Tuning the drain budget (per-listener, ROUND-8 OPS-10)

The gateway-level `[runtime].drain_timeout_ms` (default 10 s) is
correct for short-request HTTP but materially insufficient for
long-poll H1 / gRPC bidi / SSE / WebSocket listeners — Pingora ships
`EXIT_TIMEOUT=300s` for exactly this reason. Rather than raise the
gateway default (which would slow every short-request listener's
restart up to 5 min), set a **per-listener** override:

```toml
[[listeners]]
address = "0.0.0.0:443"
protocol = "h1s"            # gRPC over H2 via ALPN
drain_timeout_ms = 300000   # 5 min — Pingora EXIT_TIMEOUT
drain_jitter_ms  = 0        # optional: disable jitter for this listener
```

| Workload                                       | Recommended `drain_timeout_ms` | Rationale                          |
|------------------------------------------------|--------------------------------|------------------------------------|
| Short-request HTTP (request/response, p99 <1s) | 10000 (default)                | Pingora `CLOSE_TIMEOUT` analogue.  |
| Streaming HTTP / SSE / long-poll (p99 <60s)    | 60000                          | Allow one full poll cycle.         |
| gRPC bidi / WebSocket                          | 300000                         | Pingora `EXIT_TIMEOUT` default.    |

`None` (key omitted) inherits `[runtime].drain_timeout_ms`. Validation
range `100..=300000` ms (same as the gateway key). The coordinator's
InFlightDrain phase waits the **max** effective per-listener budget so
the longest-budget listener gets its full window;
`lb_drain_timeout_ms_listener{listener}` on `/metrics` reflects the
effective per-listener budget (used by `LbShutdownSlow`).

Calibrate with `shutdown_drain_seconds_listener{phase="InFlightDrain",
listener=...}` p99: if it routinely reaches the budget the drain is
*timing out* (raise the per-listener budget); if it sits below
`drain_timeout_ms / 10` the budget is over-generous (lower it to keep
deploys snappy).

### Drain jitter (ROUND-8 OPS-02)

On a deploy-wide SIGTERM every replica's drain coordinator would
otherwise fire its in-flight cancel at the same wall-clock instant,
producing a thundering-herd reconnect storm against the shared
upstream LB (Envoy hit this in production with stateful upstream LBs
at >2-3 replicas — `drain_manager_impl.cc`). The coordinator sleeps a
per-process random duration uniformly distributed in
`[0, drain_jitter_ms)` before the InFlightDrain cancel so close
events spread across the fleet instead of synchronising.

- `[runtime].drain_jitter_ms` — gateway-level ceiling. Omit (the
  default) to *derive* `drain_timeout_ms / 4` (Envoy's "first
  quarter" recipe). Set to `0` for single-instance / deterministic
  testing. Per-listener override: `[[listeners]].drain_jitter_ms`.
- Validation: an explicit value must be `<=` the effective drain
  budget it subdivides (checked gateway-level and per-listener after
  `[runtime]` inheritance is resolved).

The per-process draw desynchronises *across* replicas (the primary
thundering-herd mitigation). Per-connection intra-pod spreading (each
connection drawing its own jitter in the per-conn cancel arm) is a
tracked follow-up owned by the L7 accept/serve-connection work; it
refines the within-pod close distribution but is not required for
the cross-replica herd protection.

**Per-protocol drain signal**:

| Protocol | Drain signal               | Wired through main.rs?            |
|----------|----------------------------|-----------------------------------|
| H1       | `Connection: close`        | Pending (PROTO-2-09 — Wave 2c)    |
| H2       | `GOAWAY (NO_ERROR 0x0)`    | Pending (PROTO-2-11 H2 — Wave 2c) |
| H3       | `CONNECTION_CLOSE H3_NO_ERROR (0x0100)` | Pending (listener-token plumb)   |
| L4 TCP   | accept loop abort + cancellation token | shipped                |
| QUIC     | `QuicListener::shutdown()` join (2 s)   | shipped                |

The PROTO-2-09 / PROTO-2-11 protocol-level emission code already exists
in `lb-quic` (H3) and ships with PROTO-2-11 H2 / PROTO-2-09 H1; what
remains is plumbing `lb_core::Shutdown::token()` into the per-connection
serve loops. The integration tests `tests/reload_zero_drop.rs::test_sigterm_drains_*`
are `#[ignore]`'d until that plumbing lands.

## Health probe endpoints

The admin HTTP listener exposes four GET-only probes:
`/livez`, `/readyz`, `/startupz`, and `/healthz`. As of REL-2-04,
`/healthz` is a back-compat alias for `/livez` and returns the same
JSON body, `{"status":"<state>"}\n` (`<state>` is one of `ok` /
`starting` / `draining`), with `Content-Type: application/json;
charset=utf-8`. **Compat note for operators**: prior releases served
`/healthz` as `text/plain` with the literal body `ok\n`. Any external
liveness check that string-matched on `ok\n` (curl piped into `grep
-Fx`, shell `[[ "$body" == "ok" ]]` after `tr -d '\n'`, etc.) must be
updated to either match the JSON shape (`grep '"status":"ok"'`) or
rely on the HTTP status code, which remains `200` while the runtime
is alive. The endpoint path, port, method, and 200-while-live
semantics are unchanged — only the response body and Content-Type
moved from plain text to JSON.

## Configuration reload

```
systemctl reload expressgateway
```

`ExecReload=/bin/kill -HUP $MAINPID` dispatches **SIGHUP**, which runs the
validate-first config hot reload (S37-C): the binary re-reads the TOML,
parses + validates it, and — only if valid — applies the **swappable**
subset live through the `ConfigManager` + atomic `ArcSwap`. The reload is
governed by an honesty contract; restart-required changes are logged and
**not** silently applied.

Watch the logs after a reload:
- Success: `SIGHUP config reload pass complete`; a swappable change logs the
  per-field `SIGHUP: L7 proxy rebuilt + swapped` line and bumps
  `config_reload_succeeded_total` + `config_reload_applied_swappable_total`.
- A restart-required field logs `SIGHUP: <field> requires a restart …` and
  bumps `config_reload_restart_required_fields_total` (the live config keeps
  serving).
- Invalid config: `SIGHUP: … rolling back, keeping live config` and
  `config_reload_failed_total` — nothing is applied.

**Applied live by SIGHUP** (swappable): the `[[listeners.backends]]` pool
and the per-listener `[listeners.http]` timeouts.

**Requires a full restart** (restart-required — logged on SIGHUP, not
applied): `protocol`, listener `address`/port (socket bound at startup),
`[listeners.tls]`, `[listeners.quic]`, `max_requests_per_h3_connection`,
all `drain_*`, `[admin]`, `[security]`, `[passthrough]`, all `[runtime]`/XDP
fields, and the kernel I/O backend (`io_uring` ↔ `epoll`). **TLS cert/key
files** rotate via **SIGUSR1**, not SIGHUP (see below).

## TLS certificate rotation

REL-2-03 lands the `Arc<ArcSwap<TlsConfigBundle>>` hot-reload path
(see `crates/lb-security/src/ticket.rs` `SharedTlsBundle`); SIGUSR1
atomically swaps in a freshly-built bundle and on parse/validate
failure the old bundle stays live. To rotate certs without a
restart:

1. Write the new PEM files into `/etc/expressgateway/tls/` atomically
   (rename, don't truncate).
2. `kill -USR1 $(pidof expressgateway)` (or `systemctl reload
   expressgateway` if the unit's `ExecReload` is mapped to SIGUSR1).

The in-process **ticket-key rotator**
(`crates/lb-security/src/ticket.rs::TicketRotator`) rotates the TLS
resumption key daily by default with a 24 h overlap window. Encrypted
tickets from the previous key continue to decrypt for that overlap so
restarts don't invalidate active TLS sessions globally.

For emergency ticket-key purge (suspected key compromise) restart with
a short `rotation_interval`, wait one cycle, restore to daily.

## Reading logs

Default log level is `info`. Override at startup via
`Environment=RUST_LOG=debug` in the unit file. Log output defaults to
**JSON**; set `LB_LOG_FORMAT=text` (or `plain`) for human-readable text.

Useful filters:

- `journalctl -u expressgateway --since '5 min ago'`
- `journalctl -u expressgateway -p err` — errors and worse.
- `journalctl -u expressgateway -g 'backend pool'` — pool lifecycle.
- `journalctl -u expressgateway -g 'DNS resolver'` — resolver state.
- `journalctl -u expressgateway -g 'lifecycle signal'` — every drain
  / reload tick.
- `journalctl -u expressgateway -g 'panic'` — every caught panic
  (the panic hook bumps `panic_total` and emits one structured line).

## Alert catalog

Every alert below points at a metric family enumerated in `METRICS.md`.
Wired = the metric is being emitted today; Pending = the metric name
is reserved but not yet observed.

### LbPanic — process-level panic

**Trigger**: `rate(panic_total[5m]) > 0`
**Severity**: page
**Wired**: yes (CODE-2-02 panic hook; `metrics.panic_total_counter()`)
**Diagnose**:
1. `journalctl -u expressgateway -g 'panic'` — find the structured
   line with `location=<file:line>` and `backtrace=<...>`.
2. Confirm the location is binary code (`crates/lb/src/main.rs`); all
   libraries `#![deny(clippy::panic)]`. If it's library code, the
   deny-lint is missing — file a bug.
3. If `panic = "unwind"` (current default), tokio caught it and the
   process kept running; if `panic = "abort"`, systemd restarted.
4. Capture the panic stack trace and the last 200 lines of log; file
   an issue.

### LbShutdownAborted — drain deadline blown

**Trigger**: `increase(shutdown_aborted_connections_total[1h]) > 0`
**Severity**: warn
**Wired**: yes (`crates/lb/src/main.rs` drain path).
**Meaning**: a deploy finished with stragglers that the deadline did
not reach. Per-connection tasks were SIGKILL-equivalent aborted.
**Diagnose**:
1. Check `[runtime].drain_timeout_ms` vs your typical request latency
   p99. If long-poll/streaming workloads dominate, raise the budget.
2. Grep for `drain deadline elapsed remaining=N`; large `N` means a
   protocol with no drain signal (see the matrix above) was carrying
   live work.
3. If the increase happens outside deploys, the binary is restarting
   unexpectedly — cross-reference `LbPanic`.

### LbShutdownSlow — drains routinely brush the budget

**Trigger**:
```promql
histogram_quantile(0.99,
  sum by (le, listener) (
    rate(shutdown_drain_seconds_listener_bucket{phase="InFlightDrain"}[15m])
  )
) > 0.8 * (
  max by (listener) (lb_drain_timeout_ms_listener) / 1000
)
```
**Severity**: warn
**Wired**: yes — `shutdown_drain_seconds_{global,listener}` is emitted
per drain phase by the `MetricsDrainObserver` in
`crates/lb/src/main.rs` (ROUND-8 OPS-04+L4-12 coordinator; histogram
contract is OPS-03). `lb_drain_timeout_ms_listener` is the
build-info-style per-listener-budget gauge (ROUND-8 OPS-10).
**Meaning**: the p99 in-flight-drain phase is within 20 % of the
configured budget — drains are *completing* but only just; a small
latency regression will tip them into truncating connections
(`LbShutdownAborted`).
**Diagnose**:
1. Identify the listener via the `listener` label.
2. Compare the p99 to that listener's effective budget
   (`lb_drain_timeout_ms_listener{listener=...}`). If the listener
   carries streaming/long-poll traffic, raise its per-listener
   `[[listeners]].drain_timeout_ms` (ROUND-8 OPS-10) — see "Tuning
   the drain budget" below.
3. Histogram families: `shutdown_drain_seconds_global{phase,outcome}`
   covers ReadinessSettle / XdpDetach / Total;
   `shutdown_drain_seconds_listener{phase,outcome,listener}` covers
   ListenerCancel / InFlightDrain. Two MetricVecs by design so the
   non-listener-scoped phases don't carry an empty `listener` label.

### LbShutdownTruncatedStreams — streaming listener aborting on drain

**Trigger**: `sum by (listener) (rate(shutdown_aborted_connections_total[1h])) > 0`
for a listener carrying streaming (gRPC bidi / WebSocket / SSE)
traffic.
**Severity**: warn
**Wired**: partial — `shutdown_aborted_connections_total` is bumped
from the drain path; the streaming-listener correlation is operator
judgement (cross-reference the listener's `protocol` and the
`lb_drain_timeout_ms_listener{listener}` budget).
**Meaning**: a streaming listener is aborting connections at drain —
almost always an under-budget drain for that workload.
**Diagnose**:
1. Confirm the listener carries streaming traffic (gRPC bidi /
   WebSocket / long-poll SSE).
2. Check `lb_drain_timeout_ms_listener{listener=...}` — if it is the
   gateway default (10000) for a streaming listener, set a
   per-listener `[[listeners]].drain_timeout_ms` (300000 for gRPC
   bidi / WS, 60000 for SSE / long-poll — see "Tuning the drain
   budget" above).
3. Re-check after the next deploy that the abort rate drops to 0.

### LbAcceptSaturation — listener accept queue near cap

**Trigger**: `accept_inflight / max_inflight > 0.8 for 2m`
**Severity**: warn
**Wired**: yes — `accept_inflight{listener}` is emitted (REL-2-09); compare it
against the configured `max_inflight_connections` (the denominator is config,
not a metric).
**Diagnose**:
1. Identify the saturated listener via the `listener` label.
2. `lsof -p $(pidof expressgateway) | wc -l` vs `LimitNOFILE` —
   approaching the limit is the proximate cause.
3. Check upstream backend latency; a slow pool keeps `inflight` high.
4. Scale replicas or raise `[listeners.*].max_inflight` if the
   backend can absorb.

### LbAcceptErrors — accept(2) failing in a tight loop

**Trigger**: `rate(accept_errors_total[1m]) > 10`
**Severity**: page (EMFILE risk)
**Wired**: yes (`accept_errors_total{listener,kind}`; REL-2-10).
**Kinds** (`accept_errors_total{listener, kind}`): `emfile`, `enfile`,
`eintr`, `econnaborted`, `eagain`, `other`.
**Diagnose**:
1. `lsof -p $(pidof expressgateway) | wc -l` vs the soft fd limit.
2. Raise `LimitNOFILE` if close to cap; investigate FD leak in pool
   (`pool_idle_gauge` rising without `pool_acquires_total` matching
   release).
3. The handler implements exponential backoff (start 1 ms, cap 1 s)
   on persistent error kinds, so the CPU should not spin even while
   the alert fires.

### LbAcceptShed — accept shedding kicked in

**Trigger**: `rate(accept_shed_total[5m]) > 0`
**Severity**: warn
**Wired**: yes (`accept_shed_total{listener}`; REL-2-09 partner). Counter
increments when the listener inflight cap rejects an accept.
**Diagnose**: pair with `LbAcceptSaturation` above; same triage.

### LbXdpConntrackFull — XDP flow-table at cap

**Trigger**: `increase(xdp_conntrack_full_total{family=~"v4|v6"}[5m]) > 0`
**Severity**: warn
**Wired**: pending (REL-2-12; metric name reserved in `label_budget.rs`).
**Diagnose**:
1. `xdp_conntrack_entries_current{family}` vs
   `xdp_conntrack_capacity{family}` — confirm the map is at limit.
2. If sustained, raise the map size in the BPF source and reload, or
   shorten the entry TTL. CONNTRACK saturation degrades silently:
   new flows fall back to the kernel TCP stack.

### LbXdpAttachMode — XDP attached in unexpected mode

**Trigger**: `xdp_attached_mode{mode="drv"} == 0 AND xdp_attached_mode{mode="skb"} == 1`
**Severity**: info
**Wired**: yes (xdp_metrics.rs).
**Meaning**: the loader fell back from native (`drv`) to generic
(`skb`) XDP. Performance penalty is significant.
**Diagnose**:
1. `ip link show dev <iface>` — check the driver name.
2. Check kernel logs for verifier rejections.
3. Some virtual NICs (virtio, veth) only support `skb`. Acceptable
   on those.
4. **AWS `ena` driver:** native (`drv`) attach is refused unless
   `MTU ≤ 3498` AND `combined channels ≤ max/2`, because the shipped
   `lb_xdp.bin` is built without XDP multi-buffer/frags. `dmesg` will
   show `MTU (...) is larger than the maximum allowed MTU (3498)
   while xdp is on` or `channel count should be at most half`. Fix
   the MTU/channels per `DEPLOYMENT.md` § "ENA native-XDP
   requirements" — `skb` fallback here is a *configuration* gap, not
   a hardware limit.

### LbXdpSamplerErrors — userspace BPF map read failing

**Trigger**: `rate(xdp_sampler_errors_total[5m]) > 0`
**Severity**: warn
**Wired**: yes (xdp_metrics.rs). `kind` label distinguishes
`map_lookup` / `permission` / `other`.
**Diagnose**: usually a permissions issue (`CAP_BPF` missing) or the
ELF was rebuilt without updating the loader's map names.

### LbCertRotationFailed — TicketRotator failed to rotate

**Trigger**: `increase(cert_rotation_failed_total[1h]) > 0`
**Severity**: page (TLS resumption stops working after `overlap`)
**Wired**: pending (REL-2-03 / SEC-2-04 wiring).
**Diagnose**:
1. `journalctl -u expressgateway -g 'rotation'` — look for the
   failure reason from `TicketRotator::rotate_if_due`.
2. If filesystem-backed, check `/etc/expressgateway/tls/` permissions
   and free space.

### LbReqDuration — request latency p99 high

**Trigger**: `histogram_quantile(0.99, sum by (le, listener, version) (rate(http_request_duration_seconds_bucket[5m]))) > 1.0`
**Severity**: warn
**Wired**: yes (per-connection histogram; per-request latency hook is
the lb-l7 follow-up in `METRICS.md`).
**Diagnose**:
1. Identify the slow listener / version from the emitted
   `http_request_duration_seconds` (labelled `listener`, `version`).
2. Split gateway-local vs upstream latency: the per-backend
   `backend_request_duration_seconds` is pending (not yet emitted), so today
   use the backend's own latency metrics/logs alongside the gateway figure.
3. `pool_acquires_total` vs `pool_probe_failures_total` — pool churn
   adds connect-time per request.

### LbReq5xx — 5xx rate elevated

**Trigger**: `sum by (listener) (rate(http_requests_total{status_class="5xx"}[5m])) / sum by (listener) (rate(http_requests_total[5m])) > 0.05`
**Severity**: warn
**Wired**: yes.
**Diagnose**: today, attribute the 5xx with the emitted frontend
`http_requests_total{listener,status_class="5xx"}` (per listener) plus the
backend logs. Per-backend attribution via `backend_requests_total{backend,
status_class="5xx"}` is pending (not yet emitted).

### LbDnsCacheMiss — DNS cache thrashing

**Trigger**: `rate(dns_cache_misses_total[5m]) / (rate(dns_cache_hits_total[5m]) + rate(dns_cache_misses_total[5m])) > 0.5`
**Severity**: warn
**Wired**: yes (delta-inferred at the `spawn_tcp` site).
**Diagnose**:
1. Are backends defined by hostname or IP? Hostname backends
   re-resolve on TTL expiry.
2. Is the resolver returning short TTLs? Check upstream DNS.
3. Negative cache TTL is fixed at 5 s; persistent NXDOMAIN floods
   are visible here.

### LbPoolProbeFailures — pool liveness probes rejecting idle conns

**Trigger**: `rate(pool_probe_failures_total[5m]) / rate(pool_acquires_total[5m]) > 0.1`
**Severity**: warn
**Wired**: yes.
**Meaning**: the Pingora EC-01 pre-reuse liveness probe is discarding
more than 10 % of cached connections. Network or middlebox is
half-closing idle connections faster than `idle_timeout`.
**Diagnose**: shorten `[pool].idle_timeout`, or set
`[pool].per_peer_max=1` to disable pooling for the affected backend.

### LbConnectionsInflight — too many live connections per listener

**Trigger**: `connections_inflight > 0.9 * <local-listener-cap>`
**Severity**: warn
**Wired**: pending (reserved in `label_budget`; not yet emitted). Until it
lands, use `accept_inflight` (emitted) as the live saturation signal — see
`LbAcceptSaturation`.
**Diagnose**: paired with `LbAcceptSaturation`; same triage.

## XDP diagnosis

Preconditions:
- BPF ELF (`lb_xdp.bin`) exists — see `DEPLOYMENT.md` XDP toolchain
  caveat.
- Process has the capability set documented in `DEPLOYMENT.md`
  (`CAP_BPF` + `CAP_NET_ADMIN` on ≥ 5.8; or `CAP_SYS_ADMIN` +
  `CAP_NET_ADMIN` on < 5.8).

Inspect loaded programs:
```
sudo bpftool prog show | grep xdp_lb
sudo bpftool prog dump xlated id <id>
sudo bpftool map show | grep -E 'CONNTRACK|L7_PORTS|ACL_DENY|STATS'
```

Check an XDP program is attached to an interface:
```
ip link show dev <iface>
# Look for 'xdp', 'xdpgeneric', or 'xdpdrv' on the link.
```

Detach manually (if the binary died without cleanup):
```
sudo ip link set dev <iface> xdp off
```

Loader-error catalog (`crates/lb-l4-xdp/src/loader.rs::XdpLoaderError`):
- `InvalidElf` — bundled ELF corrupt or built for the wrong target.
  Rebuild.
- `Parse(_)` — aya failed to parse. Check LLVM version.
- `Attach(_)` — verifier rejected the program or the interface
  doesn't support the requested mode. Retry with `XdpMode::Skb`
  (generic) to isolate.
- `MapMissing(_)` — map-name mismatch between ELF and userspace.
- `LicenseMissing` — `.license` section is not `"GPL"` (SEC-2-12
  guard).

Verifier-log capture:
```
sudo bpftool prog load crates/lb-l4-xdp/src/lb_xdp.bin /sys/fs/bpf/lb_xdp 2>&1
```

### Known-bad NIC + firmware for native XDP (ROUND8-L4-05)

Some NIC driver + firmware combinations *silently drop* packets in
native (`drv`) XDP mode: the attach syscall succeeds, every map
operation reports success, the `xdp_attach_mode` gauge reads `drv`,
and the packet path goes to /dev/null. This is the aya #1193 /
Cilium-lesson-8 silent-drop class.

ExpressGateway refuses `drv` on these combinations *before*
attempting the attach (a failed-attach check is not enough — the
attach "succeeds"). The static blocklist
(`crates/lb-l4-xdp/src/nic_compat.rs::BLOCKLIST`):

| Driver      | Unsafe firmware       | Action                |
|-------------|-----------------------|-----------------------|
| `mlx5_core` | `< 16.32.1010`        | demote `drv` → `skb`  |
| `ena`       | `< 2.10` (c5n/m5n)    | demote `drv` → `skb`  |
| `ice`       | `<= 4.10`             | demote `drv` → `skb`  |

When a row matches, the loader logs a loud `WARN` ("xdp Drv refused
by NIC blocklist…"), increments `xdp_attach_probe_failed_total`, and
continues the ladder to `skb`.

Operator override (force generic mode regardless of NIC):
```toml
[runtime]
xdp_mode = "skb"
```

Diagnose a suspected silent drop:
```
ethtool -i <iface> | grep -E 'driver|firmware-version'
# Cross-check against the table above.

# Is the data plane actually forwarding? STAT_TX_V4 must climb:
sudo bpftool map dump name stats   # slot 5 = tx_v4
# xdp_attach_mode == "drv" but tx_v4 flat under load == silent drop.
```

The static blocklist is best-effort and will go stale. The always-on
backstop is a post-attach `BPF_PROG_TEST_RUN` synthetic-packet probe;
its kernel-touching implementation is deferred (aya 0.13.1 exposes no
public `BPF_PROG_TEST_RUN` wrapper — see the API-blocker note in
`nic_compat.rs`). The CI privileged stage runs the probe fixture and
alerts on `xdp_attach_probe_failed_total > 0`.

## DNS cache inspection

The resolver keeps its cache in-memory
(`crates/lb-io/src/dns.rs::DnsResolver`). There is no external inspect
endpoint today. Debug methods (`cache_size`, `refresh_all`) are
test-only.

To force a refresh, restart the service.

## Pool diagnostics

`TcpPool` exposes `idle_count()` only programmatically; no admin
endpoint today. `pool_idle_gauge` is sampled once per second.

Diagnose elevated new-connection rate to backends:
- Check `per_peer_max` — low values starve reuse.
- Check `idle_timeout` / `max_age` — values shorter than request
  inter-arrival times force churn.
- Liveness-probe discards: see `LbPoolProbeFailures` above.

## Panics

Library crates `#![deny(clippy::unwrap_used, clippy::expect_used,
clippy::panic, ...)]` so every panic originates in the binary or in
test-only code. The binary installs a `std::panic::set_hook` at
startup that:

1. Bumps the registry-backed `panic_total` counter.
2. Emits a single structured `tracing::error!` with `location` and
   `backtrace` fields.
3. Returns; the runtime catches the unwind in the offending task and
   the rest of the process keeps serving.

If you see a panic in logs, file an issue with the stack trace and
the last 200 lines of log.

## Version check

```
expressgateway --version     # not implemented; the binary logs its
                             # version at startup.
strings /usr/local/bin/expressgateway | grep ExpressGateway
journalctl -u expressgateway -r | head -20    # most recent startup
```

## Triage matrix (alert → first command)

| Alert                       | First command                                                   |
|-----------------------------|-----------------------------------------------------------------|
| `LbPanic`                   | `journalctl -u expressgateway -g 'panic' -n 50`                 |
| `LbShutdownAborted`         | `journalctl -u expressgateway -g 'drain deadline elapsed'`      |
| `LbShutdownSlow`            | `curl 127.0.0.1:9090/metrics \| grep shutdown_drain_seconds`    |
| `LbShutdownTruncatedStreams`| `curl 127.0.0.1:9090/metrics \| grep lb_drain_timeout_ms_listener` |
| `LbAcceptSaturation`        | `lsof -p $(pidof expressgateway) \| wc -l`                       |
| `LbAcceptErrors`            | `journalctl -u expressgateway -g 'accept error'`                |
| `LbAcceptShed`              | Same as `LbAcceptSaturation`                                    |
| `LbXdpConntrackFull`        | `bpftool map show \| grep CONNTRACK`                            |
| `LbXdpAttachMode`           | `ip link show dev <iface>`                                      |
| `LbXdpSamplerErrors`        | `journalctl -u expressgateway -g 'xdp sampler'`                 |
| `LbCertRotationFailed`      | `journalctl -u expressgateway -g 'rotation'`                    |
| `LbReqDuration`             | `curl 127.0.0.1:9090/metrics \| grep http_request_duration_seconds` |
| `LbReq5xx`                  | `curl 127.0.0.1:9090/metrics \| grep '_total{.*5xx'`            |
| `LbDnsCacheMiss`            | `curl 127.0.0.1:9090/metrics \| grep dns_cache`                 |
| `LbPoolProbeFailures`       | `curl 127.0.0.1:9090/metrics \| grep pool_`                     |
| `LbConnectionsInflight`     | `curl 127.0.0.1:9090/metrics \| grep accept_inflight` (connections_inflight pending) |
