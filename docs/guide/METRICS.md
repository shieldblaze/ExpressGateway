# Metrics

This document inventories the metrics ExpressGateway exposes today, the metrics PROMPT.md §18 specifies, the deltas, and how to scrape them.

## Present-day surface

As of Task #21 the registry is backed by `prometheus::Registry` with a handle cache for get-or-create semantics. The shape is defined in `crates/lb-observability/src/lib.rs`:

```rust
pub struct MetricsRegistry {
    inner: prometheus::Registry,
    handles: DashMap<String, Handle>,
}
```

Public methods:
- `MetricsRegistry::new() -> Self`
- `counter(name, help) -> IntCounter`
- `counter_vec(name, help, labels) -> IntCounterVec`
- `histogram(name, help, buckets) -> Histogram`
- `histogram_vec(name, help, labels, buckets) -> HistogramVec`
- `gauge(name, help) -> IntGauge`
- `gather() -> Vec<MetricFamily>`
- back-compat `increment(name, value)` / `get(name) -> Option<u64>` — route to the same underlying `IntCounter`.

Repeat `counter("foo", "...")` calls return the same handle via the name-keyed cache, so instrumentation sites can fetch their metric on the hot path without external caching. A soft cardinality guard warns at 10 000 series.

### Prometheus exposition

`crates/lb-observability/src/prometheus_exposition.rs::render_text` serializes the registry in Prometheus text format (version 0.0.4). The admin listener in `crates/lb-observability/src/admin_http.rs` binds a hyper-http1 server that serves:

- `GET /metrics` — text exposition, `Content-Type: text/plain; version=0.0.4; charset=utf-8`.
- `GET /healthz` — returns 200 OK.
- Other paths/methods return 404/405.

Configured via the new `[observability]` TOML block:

```toml
[observability]
metrics_bind = "127.0.0.1:9090"
```

Loopback-only is the expected posture; there is no built-in mTLS yet. The listener is spawned from `crates/lb/src/main.rs::async_main` when `metrics_bind` is `Some`, and cancels cleanly on SIGINT/SIGTERM.

### Instrumentation points wired in `lb/src/main.rs`

| Metric | Kind | Labels | Site |
|--------|------|--------|------|
| `pool_acquires_total` | Counter | — | Every `proxy_connection` attempt. |
| `pool_probe_failures_total` | Counter | — | `proxy_connection` when `TcpPool::acquire` errors. |
| `pool_idle_gauge` | Gauge | — | Sampled once per second via the background sampler (`TcpPool::idle_count`). |
| `dns_cache_entries` | Gauge | — | Same sampler (`DnsResolver::cache_size`). |
| `dns_cache_hits_total` / `dns_cache_misses_total` | Counter | — | `spawn_tcp` compares `cache_size` before/after `DnsResolver::resolve` to infer hit vs. miss for each backend entry at startup/reload. |
| `http_requests_total` | CounterVec | `listener`, `route`, `version`, `status_class` | Per-connection at the L7 dispatch site inside `run_listener` (`listener` = bind address; `route` is emitted empty today). |
| `http_request_duration_seconds` | HistogramVec | `listener`, `route`, `version` | Observed from accept to `serve_connection` return. |

These are intentionally a small set — per-request HTTP telemetry will land when lb-l7 grows a stats hook. The remaining `❌` rows below stay deferred.

## PROMPT.md §18 specification

The spec lists 30+ metrics across five families. Status summary (present = name appears in source; ready = exposed through a Prometheus endpoint).

### L4 XDP (`lb-l4-xdp`)

| Metric | PROMPT.md name | Present | Ready | Notes |
|--------|----------------|:-------:|:-----:|-------|
| Packets forwarded | `xdp.packets.forwarded` | ❌ | ❌ | BPF `STATS` map exists (`crates/lb-l4-xdp/ebpf/src/main.rs`); userspace pull is Pillar 4b. |
| Packets dropped | `xdp.packets.dropped` | ❌ | ❌ | Same as above. |
| Flow-table entries | `xdp.conntrack.entries` | ❌ | ❌ | `ConntrackTable::len()` exists (sim); BPF map read is Pillar 4b. |

### L7 protocol counters (`lb-h1`, `lb-h2`, `lb-h3`)

| Metric | Present | Ready | Notes |
|--------|:-------:|:-----:|-------|
| `http_requests_total{listener,route,version,status_class}` | partial | ✅ | Wired per-connection in `run_listener`; per-request granularity requires a lb-l7 hook. |
| `http_request_duration_seconds{listener,route,version}` | partial | ✅ | Histogram observed from accept to `serve_connection` return (connection scope). |
| `http2.streams.active` | ❌ | ❌ | Requires live H2 state machine. |
| `http2.settings.rejected` | ❌ | ❌ | `SettingsFloodDetector` exists but is not yet invoked. |
| `http3.qpack.decoder_stream_bytes` | ❌ | ❌ | Requires live H3 session. |

### Pool (`lb-io::pool`)

| Metric | Present | Ready | Notes |
|--------|:-------:|:-----:|-------|
| `pool_idle_gauge` | ✅ | ✅ | Sampled once/sec from `TcpPool::idle_count()`. |
| `pool_acquires_total` | ✅ | ✅ | Incremented on every `proxy_connection` acquire. |
| `pool_probe_failures_total` | ✅ | ✅ | Incremented when `TcpPool::acquire` returns an error. |
| `pool.evictions.expired` | ❌ | ❌ | Add in `PooledTcp::drop` eviction branch (requires lb-io hook). |

### DNS (`lb-io::dns`)

| Metric | Present | Ready | Notes |
|--------|:-------:|:-----:|-------|
| `dns_cache_hits_total` / `dns_cache_misses_total` | ✅ | ✅ | Inferred from `cache_size` delta at the call site in `spawn_tcp`. |
| `dns.nxdomain.cached` | ❌ | ❌ | |
| `dns_cache_entries` | ✅ | ✅ | Sampled once/sec from `DnsResolver::cache_size()`. |

### Security (`lb-security`, `lb-h2::security`)

| Metric | Present | Ready | Notes |
|--------|:-------:|:-----:|-------|
| `security.rapid_reset.tripped` | ❌ | ❌ | Detector returns `Result`; wiring site counts and emits. |
| `security.continuation_flood.tripped` | ❌ | ❌ | Same. |
| `security.hpack_bomb.tripped` | ❌ | ❌ | Same. |
| `security.slowloris.reaped` | ❌ | ❌ | |
| `security.zero_rtt.replay_rejected` | ❌ | ❌ | |
| `tls.ticket_key.rotations` | ❌ | ❌ | `TicketRotator::rotate_if_due` returns `bool`; wiring counts. |

## Prometheus exposition (historical notes)

Shipped in Task #21. See the "Present-day surface" section above for the concrete endpoint + wiring. Remaining follow-ups:

- Per-request HTTP telemetry (`http.requests.total` by actual response status, QPS histograms with `le` buckets refined from production data).
- Security-family metrics (rapid-reset, continuation-flood, HPACK bomb trips).
- XDP counters (`xdp.packets.forwarded/dropped`, `xdp.conntrack.entries`) — blocked on Pillar 4b userspace BPF map pull.
- mTLS-protected admin listener; push-gateway support; OTLP export.

Cardinality bound: `http_requests_total{listener,route,version,status_class}` is governed by the closed-set REL-2-08 label budget (see the canonical table below). The startup `LabelBudget` check refuses to boot if the configured shape would exceed `[observability].max_label_cardinality` (default 10 000 series). `route` is emitted empty today, so the effective cardinality is `listeners × versions × status_classes`; the budget caps the listener dimension for deployments hosting many listeners.

## Sample Grafana panel

For the request-rate panel, once `http.requests.total` is emitted:

```json
{
  "type": "timeseries",
  "title": "Request rate by HTTP version",
  "datasource": "Prometheus",
  "targets": [
    {
      "expr": "sum by (version) (rate(http_requests_total[1m]))",
      "legendFormat": "HTTP/{{version}}"
    }
  ],
  "fieldConfig": {
    "defaults": { "unit": "reqps" }
  }
}
```

For a P99 latency panel once latency histograms land:

```json
{
  "type": "timeseries",
  "title": "Request latency P99",
  "datasource": "Prometheus",
  "targets": [
    {
      "expr": "histogram_quantile(0.99, sum by (le, version) (rate(http_request_duration_seconds_bucket[5m])))",
      "legendFormat": "{{version}} P99"
    }
  ],
  "fieldConfig": { "defaults": { "unit": "s" } }
}
```

## How this gap is tracked

Every `❌` row above is a metric the registry does not yet emit. As the metric surface is promoted from counters-only toward full Prometheus exposition (histograms, gauges, and labels), this file's "Ready" column fills in and the panels above can be assembled.

## REL-2-08: canonical label keys + cardinality budget

The rel-owned RED metric families settled on a closed-set label
vocabulary. The list below is the **single source of truth**;
`lb_observability::label_budget::CANONICAL_LABELS` mirrors it in
code and the integration test in
`crates/lb-observability/tests/red_label_budget.rs` re-validates
the live registry against this table on every CI run.

| Family                                     | Type        | Labels                                              | Worst case (8L × 32B × 16R)  |
|--------------------------------------------|-------------|-----------------------------------------------------|------------------------------|
| `http_requests_total`                      | CounterVec  | `listener`, `route`, `version`, `status_class`      | 8 × 16 × 4 × 5 = 2 560       |
| `http_request_duration_seconds`            | HistogramVec| `listener`, `route`, `version`                      | 8 × 16 × 4 = 512             |
| `backend_requests_total`                   | CounterVec  | `listener`, `backend`, `status_class`               | 8 × 32 × 5 = 1 280           |
| `backend_request_duration_seconds`         | HistogramVec| `listener`, `backend`                               | 8 × 32 = 256                 |
| `connections_inflight`                     | GaugeVec    | `listener`                                          | 8                            |
| `xdp_conntrack_full_total`                 | Counter     | `family` (`v4` \| `v6`)                             | 2                            |
| `xdp_conntrack_entries_current`            | Gauge       | `family`                                            | 2                            |
| `xdp_conntrack_capacity`                   | Gauge       | `family`                                            | 2                            |
| `xdp_packets_total`                        | Counter     | `action` (`pass` \| `drop` \| `tx` \| …)            | ≤ 16                         |
| `xdp_bytes_total`                          | Counter     | `direction` (`rx` \| `tx`)                          | 2                            |
| `xdp_attached_mode`                        | Gauge       | `mode` (`drv` \| `skb` \| `hw`)                     | 3                            |
| `xdp_sampler_errors_total`                 | Counter     | `kind`                                              | small                        |

### Why these and not others?

- **No `backend` label on `http_requests_total`.** Request-level
  metrics multiply listener × route × version × status_class but
  not backend; otherwise a 256-backend pool with two listeners
  already burns 25 600 series per family. Backend-keyed series
  live on the `backend_*` family instead.
- **No `status_class` label on duration histograms.** Each
  histogram already has ~12 bucket entries; adding 5× status
  classes multiplies the underlying time-series count five-fold
  for no observable benefit (operators always look at p50/p99
  irrespective of status).
- **`action` not `result` on `xdp_packets_total`.** Locked in the
  REL-2-13 / EBPF-2-08 cross-review §2.

### Startup-time cardinality budget

`lb_observability::LabelBudget::check(...)` runs at startup and
refuses to boot if any family would emit more than
`[observability].max_label_cardinality` series (default 10 000).
The check is purely arithmetic — it multiplies the static config
shape through the canonical label keys above. This is a hard rail
against operator misconfiguration that would otherwise saturate
the local Prometheus instance on the first scrape.

Error:
```
label cardinality budget exceeded: backend_requests_total would emit
up to 500000 series, ceiling is 10000
```

## Canonical Prometheus family enumeration (round-4)

The list below is the **complete** inventory of every metric the
binary will emit when fully wired. Wired = the metric is being emitted
today; Pending = the name is reserved and the alert is in `RUNBOOK.md`,
but the instrumentation site is owned by a Wave-2c follow-up. The
canonical-label table above (REL-2-08) governs label keys for the RED
families.

### Process / lifecycle

| Family                                  | Type    | Labels | Wired | Cardinality | Source                                  |
|-----------------------------------------|---------|--------|:-----:|-------------|-----------------------------------------|
| `panic_total`                           | Counter | —      |  yes  | 1           | `crates/lb/src/main.rs::panic_hook`     |
| `shutdown_aborted_connections_total`    | Counter | —      |  yes  | 1           | `crates/lb/src/main.rs` drain path      |
| `connections_total`                     | Counter | —      |  yes  | 1           | per-accept on every L7 listener         |
| `bytes_client_to_backend`               | Counter | —      |  yes  | 1           | L4 copy_bidirectional totals            |
| `bytes_backend_to_client`               | Counter | —      |  yes  | 1           | L4 copy_bidirectional totals            |

### L7 RED (canonical, REL-2-08)

| Family                              | Type         | Labels                                            | Wired   | Worst case |
|-------------------------------------|--------------|---------------------------------------------------|:-------:|------------|
| `http_requests_total`               | CounterVec   | `listener`, `route`, `version`, `status_class`    | partial | 2 560      |
| `http_request_duration_seconds`     | HistogramVec | `listener`, `route`, `version`                    | partial | 512        |
| `backend_requests_total`            | CounterVec   | `listener`, `backend`, `status_class`             | pending | 1 280      |
| `backend_request_duration_seconds`  | HistogramVec | `listener`, `backend`                             | pending | 256        |
| `connections_inflight`              | GaugeVec     | `listener`                                        | pending | 8          |

"partial" means per-connection scoping today (one observation per
`serve_connection` return); per-request granularity requires a
lb-l7 hook (REL-2-08 follow-up).

### Accept-side health (REL-2-09 / REL-2-10)

| Family                              | Type       | Labels                                            | Wired   | Cardinality |
|-------------------------------------|------------|---------------------------------------------------|:-------:|-------------|
| `accept_inflight`                   | GaugeVec   | `listener`                                        |   yes   | 8           |
| `accept_shed_total`                 | CounterVec | `listener`                                        |   yes   | 8           |
| `accept_errors_total`               | CounterVec | `listener`, `kind` (`emfile`/`enfile`/`eintr`/`econnaborted`/`eagain`/`other`) | yes | 48 |

### TCP pool (`lb-io::pool`)

| Family                              | Type     | Labels | Wired | Cardinality |
|-------------------------------------|----------|--------|:-----:|-------------|
| `pool_acquires_total`               | Counter  | —      |  yes  | 1           |
| `pool_probe_failures_total`         | Counter  | —      |  yes  | 1           |
| `pool_idle_gauge`                   | Gauge    | —      |  yes  | 1           |

### DNS (`lb-io::dns`)

| Family                              | Type    | Labels | Wired | Cardinality |
|-------------------------------------|---------|--------|:-----:|-------------|
| `dns_cache_hits_total`              | Counter | —      |  yes  | 1           |
| `dns_cache_misses_total`            | Counter | —      |  yes  | 1           |
| `dns_cache_entries`                 | Gauge   | —      |  yes  | 1           |

### XDP (REL-2-12, REL-2-13)

| Family                              | Type    | Labels                                  | Wired   | Cardinality |
|-------------------------------------|---------|-----------------------------------------|:-------:|-------------|
| `xdp_packets_total`                 | Counter | `action`                                |  yes    | ≤ 16        |
| `xdp_bytes_total`                   | Counter | `direction`                             |  yes    | 2           |
| `xdp_conntrack_full_total`          | Counter | `family`                                | pending | 2           |
| `xdp_conntrack_entries_current`     | Gauge   | `family`                                | pending | 2           |
| `xdp_conntrack_capacity`            | Gauge   | `family`                                | pending | 2           |
| `xdp_attached_mode`                 | Gauge   | `mode`                                  |  yes    | 3           |
| `xdp_sampler_errors_total`          | Counter | `kind`                                  |  yes    | small       |

### TLS / security (SEC-2 family)

| Family                              | Type    | Labels  | Wired   | Cardinality |
|-------------------------------------|---------|---------|:-------:|-------------|
| `cert_rotation_succeeded_total`     | Counter | —       |  yes    | 1           |
| `cert_rotation_failed_total`        | Counter | `reason`|  yes    | small       |
| `cert_loaded_at_seconds`            | Gauge   | `listener` | yes  | 1 / listener|
| `ticket_key_rotations_total`        | Counter | —       | pending | 1           |
| `security_rapid_reset_tripped_total`        | Counter | —       | pending | 1   |
| `security_continuation_flood_tripped_total` | Counter | —       | pending | 1   |
| `security_hpack_bomb_tripped_total`         | Counter | —       | pending | 1   |
| `security_settings_flood_tripped_total`     | Counter | —       | pending | 1   |
| `security_ping_flood_tripped_total`         | Counter | —       | pending | 1   |
| `security_zero_window_stall_total`          | Counter | —       | pending | 1   |
| `security_slowloris_reaped_total`           | Counter | —       | pending | 1   |
| `security_zero_rtt_replay_rejected_total`   | Counter | —       | pending | 1   |

### Config reload (S37-C)

Emitted by the SIGHUP validate-first config hot-reload handler in
`crates/lb/src/main.rs` (registered once at boot). See `CONFIG.md`
"Reload semantics" and `RUNBOOK.md` "Configuration reload".

| Family                                        | Type    | Labels | Wired | Cardinality |
|-----------------------------------------------|---------|--------|:-----:|-------------|
| `config_reload_succeeded_total`               | Counter | —      | yes   | 1           |
| `config_reload_failed_total`                  | Counter | —      | yes   | 1           |
| `config_reload_applied_swappable_total`       | Counter | —      | yes   | 1           |
| `config_reload_restart_required_fields_total` | Counter | —      | yes   | 1           |
| `config_reload_applied_version`               | Gauge   | —      | yes   | 1           |

### Cardinality budget summary

Sum of the worst-case columns above: ≈ 4 800 series at the spec
shape (8 listeners × 16 routes × 32 backends × 4 versions × 5 status
classes). The startup-time check (`LabelBudget::check`) defaults to
`max_label_cardinality = 10 000` so a doubling of the deployment
shape still fits before refusal.

## Scrape configuration

```yaml
# /etc/prometheus/scrape_configs.d/expressgateway.yaml
- job_name: expressgateway
  scrape_interval: 15s
  scrape_timeout: 5s
  static_configs:
    - targets: ['127.0.0.1:9090']
  metric_relabel_configs:
    # drop the dynamic `route` label if it pushes total series above
    # the local Prometheus's budget — leaves listener/version/status_class.
    - source_labels: [__name__]
      regex: 'http_(requests_total|request_duration_seconds.*)'
      target_label: route
      replacement: ''
      action: replace
```

## Doc-lint guardrails

`scripts/ci/doc-lint.sh` greps for stale references — see the script
itself for the exact pattern list. It is wired into
`.github/workflows/ci.yml` and fails the job on any hit. <!-- doc-lint-allow -->
