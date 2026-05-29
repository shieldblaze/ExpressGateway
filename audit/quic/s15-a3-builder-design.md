# S15 A3 builder design — threat-model defences + observability

Branch `quic-s15-a3-builder` (from `034ddf30`). Implements design §A3
(`audit/quic/s15-design.md` ~line 739) for the Mode A QUIC passthrough
datapath. Plan-approval gate per A2 discipline: NO `.rs` edits until the
lead approves this doc.

## 0. What is ALREADY done (do not re-implement)

Reading the current tree (`034ddf30`) the following A3 items are already
landed and only need test/coverage + audit-throttle wiring:

- **`min_client_dcid_len` floor** — `PassthroughConfig.min_client_dcid_len`
  (lb-config/src/lib.rs:959, `serde(default)` = 8, `validate` range
  `8..=20` at :1626), `PassthroughParams.min_client_dcid_len`
  (passthrough.rs:99, default 8 at :149), enforced in `handle_initial`
  (passthrough.rs:581-589 — drops below-floor Initials), and wired
  config→params in `spawn_passthrough` (main.rs:1044). **No source change
  needed; A3 only adds the table-driven unit test for it.**
- **`mint_retry` knob** + `strict_source_binding` + `audit_throttle_window`
  config/params/wiring — all present.

So the residual A3 impl scope is: **(1) Prometheus metrics**, **(2) audit-
log throttle mechanism** (the window is a knob but nothing throttles yet),
**(3) unit tests** to lift passthrough.rs coverage to ≥80% and cover each
defence.

## 1. Prometheus metrics — `quic_passthrough_*`

### 1.1 Registration site & pattern

Follow the **`XdpMetrics`** template (lb-observability/src/xdp_metrics.rs):
a subsystem struct of `prometheus` handles with a
`register(&MetricsRegistry) -> Result<Self, MetricsError>` constructor that
uses the existing get-or-create `counter()` / `gauge()` methods
(lb-observability/src/lib.rs:144,335). No new registration pattern; no
`lazy_static`/`once_cell`.

New file **`crates/lb-observability/src/passthrough_metrics.rs`**, exported
from lib.rs as `pub mod passthrough_metrics;` + a `pub use` of the struct
(mirrors how `xdp_metrics` is surfaced — verify exact re-export shape and
match it):

```rust
#[derive(Clone)]
pub struct PassthroughMetrics {
    pub flows: IntGauge,                       // current dispatch-table size
    pub flows_evicted_total: IntCounter,       // LRU evictions
    pub retry_minted_total: IntCounter,        // Retry packets minted
    pub retry_rejected_total: IntCounter,      // token verify failures
    pub header_parse_errors_total: IntCounter, // public-header parse failures
    pub backend_socket_errors_total: IntCounter, // backend send/recv errors
}
impl PassthroughMetrics {
    pub fn register(reg: &MetricsRegistry) -> Result<Self, MetricsError> { … }
}
```

Names exactly as the task spec lists them. Help strings one line each.
The gauge + 5 counters are pre-seeded to 0 by virtue of get-or-create
(gauge starts 0; counters start 0) so `/metrics` shows the rows from
spawn — matching the XdpMetrics pre-seed intent.

### 1.2 lb-quic → lb-observability dependency

`PassthroughMetrics` holds raw `prometheus` `IntGauge`/`IntCounter`
handles. To bump them at the event sites in lb-quic, lb-quic must see the
type. Dependency direction is **acyclic**: lb-observability depends on
lb-security + lb-l4-xdp (NOT lb-quic), so `lb-quic → lb-observability` is
safe (confirmed by reading both Cargo.toml). Add to
`crates/lb-quic/Cargo.toml`:

```toml
lb-observability = { path = "../lb-observability" }
```

`PassthroughParams` gains:

```rust
/// Observability handles. `None` in unit tests / when the listener is
/// spawned without a registry; the event sites become no-ops.
pub metrics: Option<lb_observability::passthrough_metrics::PassthroughMetrics>,
```

Default `None` in `PassthroughParams::new`. `RouterCtx` carries the
`Option<PassthroughMetrics>` (cloned in; handles are Arc-backed/cheap).
A tiny helper macro/fn keeps the call sites clean, e.g.
`if let Some(m) = &ctx.params.metrics { m.retry_minted_total.inc(); }`.

### 1.3 Event-site instrumentation points (file:line on current tree)

| Metric | Site | Action |
|---|---|---|
| `header_parse_errors_total` | `handle_inbound` parse `Err` arm, passthrough.rs:524-527 | `.inc()` on parse failure |
| `retry_minted_total` | `handle_initial` after successful `send_to(&out, from)` of the Retry, passthrough.rs:628-630 | `.inc()` once the Retry is sent |
| `retry_rejected_total` | `handle_initial` token-verify `Err` arm, passthrough.rs:638-641 | `.inc()` on verify failure |
| `flows` (gauge) | on insert (passthrough.rs:701) `+= n`; on eviction (`evict_oldest`, :507-512) `-= removed` | keep gauge == dispatch-table size; set to `ctx.table.len()` after each mutation for self-correction |
| `flows_evicted_total` | `evict_oldest` after removing victim keys, passthrough.rs:516 | `.inc_by(removed as u64)` (or `.inc()` per evicted flow — see Q1) |
| `backend_socket_errors_total` | forward-pump send `Err` (passthrough.rs:711-713) + `reverse_pump` recv `Err` (:730-732) + reverse send `Err` (:755) + backend `bind`/`connect` `Err` (:667-674) | `.inc()` on each backend socket error |

`flows` gauge: simplest correct approach is to `.set(ctx.table.len() as
i64)` after each table insert and after each `evict_oldest` — avoids
drift from the 1-or-2-keys-per-flow asymmetry. (The gauge counts
dispatch-table entries, consistent with `flows_len()` and the verify-gate
(iii) assertion that the gauge stays at 1 for a single migrated flow —
NOTE: gate (iii) expects the gauge == flow count; a single flow with 2
keys would read 2. **Q2 below.**)

### 1.4 main.rs wiring

In `spawn_passthrough` (main.rs:1034) add a `metrics: &Arc<MetricsRegistry>`
parameter (the registry built at main.rs:1682 is in scope at the
call-site main.rs:1948). Register once and thread into params:

```rust
let pt_metrics = lb_observability::passthrough_metrics::PassthroughMetrics::register(metrics)
    .map_err(|e| anyhow::anyhow!("passthrough metrics: {e}"))?;
params.metrics = Some(pt_metrics);
```

Pass `&metrics` at the call site (main.rs:1948).

## 2. Audit-log throttle (one line per window, not per event)

`audit_throttle_window` exists as a knob but nothing enforces it. Add a
small throttle gate so `source_binding_violation` and cap-hit eviction
emit **one** audit line per window per category, satisfying the verifier's
saturation test (10_000 events → 1 line per `audit_throttle_window_secs`).

Mechanism: a per-category `AtomicU64` holding the epoch-relative millis of
the last emitted line, on `RouterCtx`. A helper:

```rust
/// Returns true if an audit line for `last_emit` may be emitted now
/// (i.e. ≥ window since the last). Updates `last_emit` on success.
fn audit_allow(last_emit: &AtomicU64, now_ms: u64, window: Duration) -> bool
```

CAS/`fetch_update` so two threads racing the same window emit once.
Categories (each its own `AtomicU64` on `RouterCtx`, initialized to a
sentinel so the FIRST event always emits):

- `source_binding_violation` — gates the `tracing::warn!(target:
  "audit", event = "source_binding_violation", …)` line in
  `forward_short_via` (passthrough.rs:826-832).
- `eviction` — gates an `audit`-target line in `evict_oldest` on cap-hit.

Audit-line convention: design §A3 names these lines by a slash-prefixed
event string — `audit/source_binding_violation` (line 751) and
`audit/quic_passthrough_cap_hit` (line 1146). The repo has NO
`target: "audit"` tracing convention and no audit helper in
lb-observability (grepped — none exists). So the audit record is a
`tracing::warn!(event = "audit/source_binding_violation", …)` with the
slash-prefixed `event` field as the design specifies, which the verifier's
saturation test can collect/count. The `trace!` already at :827 stays for
per-event debug visibility; the throttled `warn!` is the audit record. The
cap-hit eviction line is `audit/quic_passthrough_cap_hit`.

First-event-emits + one-per-window: a window of 60s default means the
saturation test (which fires 10_000 in well under 60s) sees exactly 1.

## 3. Unit tests — coverage to ≥80% session-scope

Current passthrough.rs llvm-cov = 75.91% (498/656). Uncovered clusters:
599-674 (Retry-mint + verify + backend-socket open), 787-848 (short-header
multi-length fallback + strict-source gate + `sample_lb_scid`), 994-1037
(retry-secret loader error/NotFound edges). Table-driven `#[cfg(test)] mod`
additions in passthrough.rs (in-crate so private fns are reachable):

1. **`min_client_dcid_len` floor** (table): rows
   `(dcid_len, floor, expect_dropped)` driving `handle_initial` via a
   helper that builds a `RouterCtx` with a stub backend; assert below-floor
   is dropped (no table insert, no backend socket), at/above-floor proceeds.
2. **`mint_retry == true`** path: no-token Initial → assert a Retry packet
   is sent to `from` (capture via a loopback listener socket as the
   dataplane) and `retry_minted_total` incremented; assert NO flow inserted
   (Retry is stateless).
3. **`mint_retry == false`** forward-verbatim path: no-token Initial →
   assert NO Retry minted, flow IS created and the Initial forwarded to the
   backend (counting backend socket).
4. **token-present verify FAIL** → `retry_rejected_total` inc, dropped.
   **token-present verify OK** → flow created.
5. **`evict_oldest`** (787-848 + 507-516): seed cap+1 flows with distinct
   `last_seen_ms`; assert oldest victim removed, `flows_evicted_total` inc,
   `flows` gauge decremented; negative control: under-cap → no eviction.
6. **`forward_short` multi-length fallback + strict_source_binding**:
   drive `forward_short_via` true/false table (strict on/off × peer
   match/mismatch); drive the multi-length fallback loop by registering a
   flow with a non-default `short_dcid_len`.
7. **`load_or_generate_retry_secret`** (994-1037): NotFound→generate (temp
   dir), wrong-length→Err, correct-length→Ok. Reuses the temp-dir pattern
   already in the bounded-state tests.
8. **`hash_dcid_for_maglev` / `pick_backend`** determinism (small).
9. **audit throttle** unit: `audit_allow` returns true once then false
   within the window, true again after the window elapses (drive with an
   explicit `now_ms` so no sleep).

Most tests need an async `RouterCtx` builder; I'll add a `#[cfg(test)]`
helper `fn test_ctx(params) -> Arc<RouterCtx>` that binds a real loopback
`UdpDataplane` (the `select_dataplane` Auto tier already used by spawn) so
`send_to` works and Retry packets can be captured on a sibling socket.

Self-check coverage before hand-off via
`cargo llvm-cov -p lb-quic --all-features --lcov` filtered to
passthrough.rs DA lines (the resume's method); the verifier re-measures
`--workspace` independently.

## 4. Build / gate plan

- Iterate: `CARGO_INCREMENTAL=0 cargo test -p lb-quic -p lb --all-features`.
- Cross-crate (spans lb-quic + lb-observability + lb): workspace fmt +
  clippy + check ([[cross-crate-gate-scope]]):
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-features --all-targets -- -D warnings`
  - `cargo check --workspace --all-features`
- Export `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target`, `CARGO_INCREMENTAL=0`.
- Commit per increment (metrics; throttle; tests), push continuously.

## 5. Open questions for lead (non-blocking; will default if no answer)

- **Q1.** `flows_evicted_total` granularity: increment **per evicted flow**
  (1 per cap-hit, since v1 single-evicts one flow) vs per removed dispatch
  key (1 or 2). Default: **per flow** (`.inc()` once per `evict_oldest`
  that removed ≥1 key) — "evicted" is a flow-level event.
- **Q2.** `flows` gauge semantics: dispatch-table-entry count
  (== `flows_len()`, may be 2/flow) vs unique-flow count. Verify gate
  (iii) asserts the gauge "stays at 1" for one migrated flow — but a
  migrated flow can hold 2 keys. Default: gauge = **dispatch-table size**
  (`ctx.table.len()`), and I'll flag that gate (iii)'s "==1" holds only
  while the flow has a single key (pre server-SCID registration); if the
  verifier needs unique-flow semantics, that's a +1 atomic counter. Want
  the gate-(iii) reading reconciled now or deferred to the verifier?
- **Q3.** Audit-log convention: RESOLVED by reading the tree — no
  `target: "audit"` convention or lb-observability audit helper exists; design
  §A3 names the lines `audit/source_binding_violation` (:751) and
  `audit/quic_passthrough_cap_hit` (:1146), so I'll emit
  `tracing::warn!(event = "audit/…")`. Flagging only so the lead can
  redirect to a canonical helper if one is intended.

NOT in scope (deferred, ticket-only): per-IP Retry rate-limit (v1.1),
CF-S15-PASSTHROUGH-FEATURE-GATING-DEEP, CF-S15-PASSTHROUGH-RETRY-ODCID.
