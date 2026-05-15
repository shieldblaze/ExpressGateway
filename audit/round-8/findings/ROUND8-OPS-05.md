### ROUND8-OPS-05 — `LabelBudget::check` runs only at startup; per-emission cardinality is not bounded

Reference: `audit/round-8/research/envoy.md` "Architecture summary" + "Overload manager" (Envoy publishes a label-cardinality limit per metric family that is enforced *at emission time*, not just startup); `audit/round-8/research/nginx.md` (nginx's `large_client_header_buffers` example for "defaults tight but make the knob first-class").
Our equivalent: `crates/lb-observability/src/label_budget.rs:159-187` (`LabelBudget::check`) — pure-arithmetic worst-case check at startup. `crates/lb/src/main.rs:1469-1483` (single call site, gated on config shape). Hot-path emit sites at `crates/lb/src/main.rs:2516-2530` use `counter_vec(...).with_label_values(&[...])` directly — no per-call check that the resulting series count is below ceiling.

Severity: medium
Status:   Verified-Fixed(verifier=verify, 4c1856ea)   <!-- EnforcedLabelBudget + MAX_ROUTES_BUDGET=64 in label_budget.rs, wired call site in main.rs; red_label_budget 4/4 incl. with_label_values-cannot-bypass. See audit/round-8/verify/ops.md. -->
Status (pre-verify):   Open

Divergence:
- `LabelBudget` does its math from `cfg.listeners.len()` × `max(backends_per)` × routes × {http versions, status classes}. The check is a Cartesian-product worst case derived from static config — a one-time arithmetic guard.
- The actual emit sites (`with_label_values(&[listener, route, version, status_class])`) generate one series per *observed* label tuple. Two failure modes Envoy explicitly defends against are *not* defended here:
  1. **Runtime injection of high-cardinality label values.** `version` is bounded to a static set in our code (`"h1"`, `"h2"`, `"h3"`); `route` is currently empty (`""`); `status_class` is bounded. But there is *no compile-time enforcement* that someone refactoring the emit site won't pass `client_addr.to_string()` as `route_label`. The integration test in `crates/lb-observability/tests/red_label_budget.rs` snapshots the registry against `CANONICAL_LABELS` (good — catches schema drift) but does not assert *value cardinality* against ceilings.
  2. **`route` label is `""` today.** When the per-request route extraction lands (REL-2-08 follow-up), the cardinality budget's math at startup assumed `routes_per_listener = 1` (`crates/lb/src/main.rs:1472`). The day route extraction starts emitting actual values, every `LabelBudget` calc is wrong by the actual route fan-out factor.

Impact:
- A future PR that wires per-route metrics under the existing `http_requests_total` family will pass startup `LabelBudget::check` (since the per-listener route count isn't config-known) but blow Prometheus scrape capacity at runtime. The check is a startup-only guard against startup-only mistakes.
- A bug-class regression: any emit site that pulls a value from request-scoped data (client IP, user agent, path) silently registers unbounded series. We have no `Registry::watch` guard.

Recommendation:
1. Add `LabelBudget::observed_series(&registry) -> usize` that walks the registry and returns the total series count. Emit `metrics_series_total` gauge updated by the 1Hz sampler.
2. Add `LabelBudget::ceiling_alert_threshold` (default 80% of `max_label_cardinality`). The sampler logs `error!` and bumps `metrics_cardinality_overflow_total` when the observed count exceeds the threshold. Operators get a signal *before* Prometheus refuses the scrape.
3. The runtime side: provide a `metrics::with_bounded_labels<const N: usize>(family, values: [&str; N])` helper that hashes the value tuple and increments a per-call-site `series_observed` counter; refuses to register more than `max_label_cardinality / call_sites` distinct tuples per family. This is the per-emission gate.
4. Update `METRICS.md` to call out which families have closed-set labels (`version`, `status_class`, `family`, `action`, `mode`, `direction`, `kind`) vs open-set labels (`listener`, `route`, `backend`). For each open-set label, document the maximum allowed value cardinality (likely "N listeners as declared in config", etc.).
5. Re-validate `LabelBudget::check` to assume `routes_per_listener = MAX_ROUTES_BUDGET` (a const, e.g. 64) instead of `1` so the day route extraction lands, the budget already accommodates it.

Notes:
- Round-7 audit marked REL-2-08 Verified-Fixed-Partial because the emit site uses `version` + `status_class` only, not the full canonical schema. The cardinality story is *also* partial — the budget enforces the worst case at startup but does not enforce the actual case at runtime.
