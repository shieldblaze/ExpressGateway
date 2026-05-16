# Plan for ROUND8-L7-10 + ROUND8-L7-15 — Low-severity batch

This file batches the two low-severity L7 findings whose fixes are
mostly documentation + tightening of doc-comments and contract
tests, with no behavioural change to hot-path code.

---

## ROUND8-L7-10 — Document H1 take-and-discard upstream stream pattern; doc-comment `set_reusable` API

Finding-ref:   ROUND8-L7-10 (medium per L7's own table; reframe as low under batch policy — but per the round cross-review the severity is **medium** in `coverage-gap.md`. Note: this file batches under "lows" only L7-10 and L7-15 per the task brief; the brief explicitly lists `L7-10, L7-15` as the lows-batch occupants regardless of the medium tag in `coverage-gap.md`. Honour the brief.)
Files touched:
  - `crates/lb-l7/src/h1_proxy.rs::proxy_request` (lines 758–802) — add doc-comment block explaining the take-and-discard pattern
  - `crates/lb-io/src/pool.rs::set_reusable` (line 387–389) — add doc-comment with intended use + a `#[deny(unused)]` style note that no production caller exists today
  - `crates/lb-io/src/http2_pool.rs::send_request` — add explicit error-class match for body-length-related `STREAM_CLOSED` to drive eviction (this is the H2 cousin of the H1 take-and-discard intent)
  - new test file: `crates/lb-l7/tests/round8_body_overread.rs`

Reference: **Pingora 0.8.0 CHANGELOG** — "Ensure http1 downstream session is not reused on more body bytes than expected" + 0.6.0 "Discard extra upstream body and disable keepalive". Pingora paid for this bug twice (each direction).

Approach:
1. **Document the H1 pattern**: today `proxy_request` calls `pooled.take_stream()` immediately after acquire, which means the `PooledTcp` wrapper drops without returning anything to the pool. This is *correct* behaviour (effectively single-use), but it is by accident. Add a doc-comment block citing Pingora's lesson and warning that any refactor adding upstream H1 reuse must implement `set_reusable(false)` on body-over-read.
2. **Doc-comment `set_reusable`**: explain that the API exists for future H1-upstream-reuse implementations; today it has no production caller; do not delete without first wiring a caller (else the bug is back when reuse lands).
3. **H2 caller hardening**: in `Http2Pool::send_request`, on `Send` error, inspect the hyper error class. If `e.is_h2_error()` and the H2 error code maps to a stream-level framing error (`PROTOCOL_ERROR`, `FRAME_SIZE_ERROR`, `STREAM_CLOSED` mid-body), evict the connection (we already evict on timeout — extend the matcher).
4. **Regression test**: configure a backend that emits `Content-Length: 5` and a 10-byte body; assert that (a) the proxy returns an error to the client, (b) the H2 pool's `peers` map no longer contains the offending backend after the failed request.

Proof:
  - `round8_body_overread::h2_upstream_overread_evicts_connection` — invariant: after a backend emits CL=5 with 10-byte body, the next `acquire(addr)` returns a fresh handle.
  - `round8_body_overread::h2_upstream_underread_evicts_connection` — invariant: same for under-read (premature stream-close with bytes remaining vs CL).
  - Doc-comment changes verified by `cargo doc --no-deps` rendering plus a `doc-lint.sh` extension (cross-ref bundle B-2) that asserts the named blocks exist.

Risk / blast radius: documentation-only on H1; H2 error-class matcher widens. Low risk.

Cross-ref: L7-08 (H2 RST_STREAM CANCEL on timeout — same eviction primitive). L7-14 (CVE corpus — add over-read seed).

**Audit-failure-mode theme:**
- **Theme 1 — "Verified-Fixed" snapshot of script existence, not capability**: `set_reusable` was shipped as a public API with no caller; the audit register cannot tell "API exists" from "API used in the path it was designed for".
- **Theme 3 — Doc-vs-code claim drift**: the H1 take-and-discard pattern is undocumented; the next refactor will re-introduce the bug Pingora paid for twice.

Owner: div-l7
Lead-approval: approved 2026-05-14 team-lead-r8

---

## ROUND8-L7-15 — Edge-defaults parity table + decision records

Finding-ref:   ROUND8-L7-15 (medium per `coverage-gap.md`; low per batch policy per task brief listing)
Files touched:
  - `config/default.toml`                          (expand from 12 lines to canonical edge baseline)
  - `docs/edge-defaults.md`                        (NEW — table per `_l7_handoff.md` cross-cutting #5)
  - `audit/decisions/h2-edge-streams.md`           (NEW — document `max_concurrent_streams = 256` choice)
  - `audit/decisions/path-normalisation.md`        (NEW — overlaps with L7-13; cross-ref)
  - `crates/lb-config/src/lib.rs`                  (no change; schema already exposes the knobs)
  - new test file: `crates/lb-config/tests/round8_compile_time_defaults.rs` (pin defaults via constants table)

Reference: **Envoy edge best-practices** (`max_concurrent_streams = 100`, `initial_stream_window = 64 KiB`, `initial_connection_window = 1 MiB`, `headers_with_underscores = REJECT`) + **nginx defaults** (`http2_max_concurrent_streams = 128`, `keepalive_requests = 100`, `keepalive_timeout = 75s`, `http2_recv_timeout = 30s`, `underscores_in_headers off`).

Approach:
1. **Expand `config/default.toml`** to enumerate every security default, with one comment-line per knob citing the reference (Envoy edge or nginx default). The defaults remain as compile-time consts in code; the config file *documents* them.
2. **Create `docs/edge-defaults.md`** with the divergence table from L7-15 finding (10 rows), one column per reference.
3. **Decision record for `max_concurrent_streams = 256`**: explain the choice vs Envoy 100 / nginx 128. If the choice is deliberate (we want higher throughput), document the DoS tradeoff and the operator escape hatch. If it is an oversight, drop to 128 with a one-line CHANGELOG note.
4. **Pin defaults via test**: `round8_compile_time_defaults` asserts each constant against an explicit table — drift detection. Any future cargo upgrade that drifts a default in a dependency surfaces here.
5. **Sub-findings already owned by separate plans**: L7-05 (underscores), L7-06 (keepalive count cap), L7-07 (recv-frame timeout), L7-13 (path normalisation) each ship one row of this table. L7-15 is the meta-finding; closing all four sub-plans closes L7-15.

Proof:
  - `round8_compile_time_defaults::table_matches_constants` — invariant: every (knob_name, expected_value) pair in the test table matches the live `const` in the relevant crate. Drift fails CI.
  - `cargo doc --no-deps -p lb-config` renders the schema with each knob's default visible.
  - `docs/edge-defaults.md` table is rendered and reviewed.

Risk / blast radius: documentation + test pinning; zero behaviour change.

Cross-ref: L7-05, L7-06, L7-07, L7-12, L7-13 each contribute one row.

**Audit-failure-mode theme:**
- **Theme 3 — Doc-vs-code claim drift**: defaults are in code but not in any operator-facing document. `config/default.toml` is 12 lines and silent on every security knob.
- **Theme 4 — Multi-validator audit handoff**: the canonical edge baseline has been industry-standard for years and was never imported as a checklist by the audit. `_l7_handoff.md` cross-cutting #5 finally surfaced it; this plan closes the loop.

Owner: div-l7
Lead-approval: approved 2026-05-14 team-lead-r8
