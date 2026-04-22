# Metrics

This document inventories the metrics ExpressGateway exposes today, the metrics PROMPT.md §18 specifies, the deltas, and how to scrape them.

## Present-day surface

The runtime registry is defined in `crates/lb-observability/src/lib.rs`:

```rust
pub struct MetricsRegistry {
    counters: DashMap<String, AtomicU64>,
}
```

Methods:
- `MetricsRegistry::new() -> Self`
- `increment(&self, name: &str, value: u64)` — creates the counter on first touch.
- `get(&self, name: &str) -> Option<u64>` — snapshot read.
- `len(&self) -> usize`
- `is_empty(&self) -> bool`

This is an **in-process** counter registry. It does not currently expose histograms, gauges, or labels, and there is no Prometheus exposition endpoint — the registry is constructed in `crates/lb/src/main.rs::async_main` and shared through `ListenerState::metrics` for future instrumentation.

Counter names used by the binary today:
- `listener.connections.accepted` (incremented on each accepted connection)
- `listener.connections.active` (modeled as a separate `AtomicU64` in `ListenerState::active_connections`; not routed through the registry)
- Additional counters are added as code paths grow; grep `crates/ --include='*.rs' -E 'metrics\.(increment|get)\('` for the authoritative list.

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
| `http.requests.total` (labeled by version) | ❌ | ❌ | Codec crates have no I/O driver; wiring is Pillar 3b. |
| `http.responses.total` (labeled by status class) | ❌ | ❌ | Same. |
| `http2.streams.active` | ❌ | ❌ | Requires live H2 state machine. |
| `http2.settings.rejected` | ❌ | ❌ | `SettingsFloodDetector` exists but is not yet invoked. |
| `http3.qpack.decoder_stream_bytes` | ❌ | ❌ | Requires live H3 session. |

### Pool (`lb-io::pool`)

| Metric | Present | Ready | Notes |
|--------|:-------:|:-----:|-------|
| `pool.idle.count` | partial | ❌ | `TcpPool::idle_count()` method exists; needs registry wiring. |
| `pool.acquires.total` | ❌ | ❌ | Straightforward to add in `TcpPool::acquire`. |
| `pool.probes.failed` | ❌ | ❌ | Add in the Pingora EC-01 probe path. |
| `pool.evictions.expired` | ❌ | ❌ | Add in `PooledTcp::drop` eviction branch. |

### DNS (`lb-io::dns`)

| Metric | Present | Ready | Notes |
|--------|:-------:|:-----:|-------|
| `dns.resolve.hits` / `.misses` | ❌ | ❌ | Easy addition in `DnsResolver::resolve`. |
| `dns.nxdomain.cached` | ❌ | ❌ | |
| `dns.cache.entries` | partial | ❌ | `DnsResolver::cache_size()` method exists. |

### Security (`lb-security`, `lb-h2::security`)

| Metric | Present | Ready | Notes |
|--------|:-------:|:-----:|-------|
| `security.rapid_reset.tripped` | ❌ | ❌ | Detector returns `Result`; wiring site counts and emits. |
| `security.continuation_flood.tripped` | ❌ | ❌ | Same. |
| `security.hpack_bomb.tripped` | ❌ | ❌ | Same. |
| `security.slowloris.reaped` | ❌ | ❌ | |
| `security.zero_rtt.replay_rejected` | ❌ | ❌ | |
| `tls.ticket_key.rotations` | ❌ | ❌ | `TicketRotator::rotate_if_due` returns `bool`; wiring counts. |

## Prometheus exposition

Not shipped. A minimal exposition path looks like:

1. Add a `crates/lb-observability/src/prometheus.rs` module that renders the counters as text per the [Prometheus exposition format](https://prometheus.io/docs/instrumenting/exposition_formats/).
2. Bind an admin listener in `crates/lb/src/main.rs` on a configurable port (default `127.0.0.1:9090`, loopback-only until mTLS is added).
3. Expose `/metrics` returning the rendered text; include a `version`, a `build_info` labeled-constant, and the metric families above.
4. Document the cardinality bound: each label combination is a new time series. For `http.requests.total{version,status_class}` the bound is `3 × 5 = 15`. For per-listener metrics (e.g. `listener_address`), cardinality scales with the number of listeners; keep listener labels out of request-rate metrics if the deployment has thousands of listeners.

This is tracked as a Pillar 3b task (`expose /metrics`).

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
