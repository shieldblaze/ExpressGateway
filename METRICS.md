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
| `http_requests_total` | CounterVec | `version` (h1, h2) × `status_class` (2xx, 5xx) | Per-connection at the L7 dispatch site inside `run_listener`. |
| `http_request_duration_seconds` | HistogramVec | `version` | Observed from accept to `serve_connection` return. |

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
| `http_requests_total{version,status_class}` | partial | ✅ | Wired per-connection in `run_listener`; per-request granularity requires a lb-l7 hook. |
| `http_request_duration_seconds{version}` | partial | ✅ | Histogram observed from accept to `serve_connection` return (connection scope). |
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

Cardinality bound for today's label set: `http_requests_total{version,status_class}` has at most `2 versions × 2 status classes = 4` series. Keep listener-scoped labels OFF request-rate metrics if the deployment hosts thousands of listeners.

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

Every `❌` row above maps to a TODO in `docs/gap-analysis.md` under the "Observability" section. The **Pillar 3b** milestone promotes the registry from counters-only to full Prometheus exposition with histograms, gauges, and labels, at which point this file's "Ready" column fills in and the panels above can be assembled.
