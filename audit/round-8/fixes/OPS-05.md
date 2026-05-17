# Fix plan — OPS-05: per-emission `LabelBudget` enforcement

Owner: div-ops.
Finding: ROUND8-OPS-05 (medium) — `LabelBudget::check` is a startup-only Cartesian-product worst-case check; runtime per-emission cardinality is unbounded. Future PR that wires per-request route labels under `http_requests_total` will pass startup but blow Prometheus scrape capacity.

Coverage-gap theme cited: **Theme 1 — "Verified-Fixed" snapshot of script existence, not capability.** REL-2-08 was graded `Verified-Fixed-Partial` with the partial gap disclosed (canonical-labels reservation), but the *runtime* per-emission gate was never audited. The status note hand-waved "registry snapshot test catches schema drift" without distinguishing schema (closed-set labels) from value cardinality (open-set labels).

## A. The two-tier gate

The OPS-05 finding requires *both* a runtime gauge AND a per-emission guard. They serve different purposes.

### A.1 — Tier 1: observation gauge + threshold alert (cheap, lands first)

Add `LabelBudget::observed_series_total(&registry) -> usize` walking the registry once per second (the existing 1Hz sampler in `crates/lb-observability/src/sampler.rs`):
- Emit `metrics_series_total` gauge.
- Emit `metrics_cardinality_overflow_total` counter when `observed > 0.8 * max_label_cardinality`.
- Log `error!(observed, ceiling, "label cardinality exceeded 80% of budget")` at most once per minute.

This is a *post-hoc* signal: operators see the curve growing before Prometheus refuses the scrape.

### A.2 — Tier 2: per-emission gate (expensive, lands second)

Add a `metrics::with_bounded_labels<const N: usize>(family: &str, values: [&str; N]) -> Result<(), CardinalityErr>` helper:
- Hash the tuple `(family, values)` via `seahash` or `ahash` (already a dep; verify).
- Maintain a per-call-site `HashSet<u64>` of seen tuples.
- If `seen.len() >= per_family_ceiling`, the helper refuses to register and increments `metrics_cardinality_refused_total{family}`. The emit site receives `CardinalityErr` and chooses to drop the metric for that request (the alternative — falling back to a placeholder label like `"other"` — masks the bug and is the Envoy reference's chosen tradeoff; we adopt the same).

Per-family ceiling defaults to `max_label_cardinality / n_open_set_call_sites`. Open-set call-sites are declared in `CANONICAL_LABELS` (the table from REL-2-08). Closed-set families bypass the helper entirely.

### A.3 — `route` label assumption fix

`crates/lb/src/main.rs:1472` assumes `routes_per_listener = 1`. When route extraction lands the budget math is wrong by the actual fan-out.

Replace with `MAX_ROUTES_BUDGET` const, default 64. `LabelBudget::check` uses this as the worst-case at startup. The per-emission gate (A.2) enforces it at runtime: when route extraction lands, every emit site that includes `route` goes through `with_bounded_labels`.

## B. Open-set vs closed-set documentation

Add to `METRICS.md` a per-label table:

| Label | Type | Source | Max cardinality |
|---|---|---|---|
| `version` | closed | static enum (h1/h2/h3) | 3 |
| `status_class` | closed | response status / 100 | 6 (1xx..5xx, "other") |
| `family` | closed | static enum | ~4 |
| `action`, `mode`, `direction`, `kind` | closed | static enum | bounded per call-site |
| `listener` | open | `cfg.listeners[i].name` | `cfg.listeners.len()` (typical ≤8) |
| `route` | open | per-request route extraction | `MAX_ROUTES_BUDGET` (64) — enforced by `with_bounded_labels` |
| `backend` | open | `cfg.backends[i].name` | `cfg.backends.len()` (typical ≤64) |
| `outcome` (drain), `phase` (drain) | closed | enum from `DrainPhase`/`DrainOutcome` | 5 + 2 |

Tier 2 (A.2) applies only to open-set labels. Adding a new open-set label requires:
1. Add to `CANONICAL_LABELS` with a declared ceiling.
2. Use `with_bounded_labels` at every emit site.
3. Doc-lint (OPS-09) asserts the METRICS.md table includes the new label.

## C. Code-level changes

1. `crates/lb-observability/src/label_budget.rs`:
   - Add `observed_series_total(&Registry) -> usize`.
   - Add `with_bounded_labels<const N: usize>` helper.
   - Update `LabelBudget::check` worst-case math to use `MAX_ROUTES_BUDGET=64` instead of `1`.
2. `crates/lb-observability/src/sampler.rs`: emit `metrics_series_total` + `metrics_cardinality_overflow_total` per tick.
3. `crates/lb/src/main.rs:2516-2530` (hot-path emit site): wrap `with_label_values` in `with_bounded_labels` for any open-set label tuple. (Currently `version` + `status_class` are both closed-set; the call site only needs wrapping after the route extraction lands. We pre-stage the helper now so the future PR cannot bypass it.)
4. `METRICS.md`: the per-label table from §B.

## D. Tests

- `crates/lb-observability/tests/red_label_budget.rs::test_observed_series_under_ceiling` — drive emit sites for a known number of tuples; assert `observed_series_total` matches the expected count.
- `crates/lb-observability/tests/red_label_budget.rs::test_with_bounded_labels_refuses_overflow` — force a call-site over its per-family ceiling; assert `CardinalityErr` returned and `metrics_cardinality_refused_total` incremented.
- `crates/lb-observability/tests/red_label_budget.rs::test_canonical_labels_table_matches_metrics_md` — parse METRICS.md table, assert every label appears in `CANONICAL_LABELS` and the ceiling matches.
- Integration: `tests/metrics_cardinality_runtime.rs` — start lb, drive 1k requests with synthetic route labels, scrape `/metrics`, assert `metrics_series_total < max_label_cardinality`.

## E. REL-2-08 status

REL-2-08 today is `Verified-Fixed-Partial`. The partial gap was "emit uses version + status_class only, not full canonical schema." This plan does not close that gap (route extraction is a separate workstream). It does add the gate the future close depends on, and re-articulates the partial-status note:
> Verified-Fixed-Partial. Canonical-labels reservation + startup cardinality check shipped. Per-emission `with_bounded_labels` helper shipped (Round-8 OPS-05). Route extraction itself remains deferred; when it lands the emit-site MUST route through `with_bounded_labels` and a `metrics_cardinality_refused_total` zero-tolerance assertion lands in `tests/metrics_cardinality_runtime.rs`.

## F. Verification

- `metrics_series_total` gauge appears in `/metrics` and tracks actual series count.
- `with_bounded_labels` helper exists with unit tests for refuse-on-overflow.
- `MAX_ROUTES_BUDGET=64` const exists and `LabelBudget::check` uses it.
- METRICS.md per-label table exists; doc-lint (OPS-09) asserts code-vs-docs match.
