# Round 8 — Operability divergence summary

Author: `div-ops` (task #51), 2026-05-14. Stance: adversarial re-audit. Prior Round-7 Verdict was CONDITIONAL GO; this analysis explicitly does NOT trust Round-1..-7 "Verified-Fixed" without re-checking.

## Findings filed

| ID                 | Severity | Area                                            | Reference                                       |
|--------------------|----------|-------------------------------------------------|-------------------------------------------------|
| ROUND8-OPS-01      | high     | README FD-passing claim is aspirational         | Pingora L#14 / HAProxy / nginx hot-reload       |
| ROUND8-OPS-02      | medium   | Drain has no jitter; thundering-herd risk       | Envoy L#19 `drain_manager_impl.cc`              |
| ROUND8-OPS-03      | medium   | `shutdown_drain_seconds` histogram never landed | REL-2-02 spec line 134 + HAProxy L#20           |
| ROUND8-OPS-04      | medium   | TCP accept loop has no cancel arm               | CODE-2-03 spec; Pingora L#18                    |
| ROUND8-OPS-05      | medium   | LabelBudget startup-only, not per-emission      | Envoy cardinality model                         |
| ROUND8-OPS-06      | high     | Tracing propagation library has zero callsites  | REL-2-07 partial; Pingora / Envoy baseline      |
| ROUND8-OPS-07      | medium   | Systemd unit missing modern hardening           | systemd-analyze baseline                        |
| ROUND8-OPS-08      | low      | SBOM produced by fallback tool, not cargo-cyclonedx | Round-7 deferred-to-CI                      |
| ROUND8-OPS-09      | medium   | doc-lint catches only the bug it was born for   | HAProxy "docs as contract" model                |
| ROUND8-OPS-10      | medium   | Drain budget 10 s default below streaming workload norms | Pingora EXIT_TIMEOUT=300s              |
| ROUND8-OPS-11      | medium   | readiness_settle_ms 1 s below kubelet probe period | Envoy lameduck pattern                        |
| ROUND8-OPS-12      | low      | Container image lacks RO rootfs / OCI labels / HEALTHCHECK | Generic OCI hardening                  |

Candidates examined: 14 (12 findings filed + cert rotation race + probe state machine — both moved to `ops-resolved.md`).

## Severity roll-up

- high: 2 (OPS-01, OPS-06)
- medium: 8 (OPS-02, -03, -04, -05, -07, -09, -10, -11)
- low: 2 (OPS-08, OPS-12)

## Re-audit calls against Round-7 "Verified-Fixed" statuses

Three Round-7 closures the adversarial walk found to be partial-or-overstated:

- **REL-2-02** (drain ordering) — Round 7 closed as Verified-Fixed. ROUND8-OPS-03 finds the `shutdown_drain_seconds` histogram (explicitly required by the recommendation block) never landed. ROUND8-OPS-04 finds the TCP accept loop still uses `JoinHandle::abort()` instead of the cancel arm CODE-2-03 prescribed. Status should be Verified-Fixed-Partial.
- **REL-2-03** (cert rotation) — Round 7 closed as Verified-Fixed. Walk confirms the lock-to-bundle invariant is correct (see `ops-resolved.md` "Cert rotation locks handshake to bundle"). No re-open. Inotify trigger and not_after gauge wiring are still deferred per the round-2 status note.
- **REL-2-04** (probes) — Round 7 closed as Verified-Fixed. ROUND8-OPS-11 finds the `readiness_settle_ms` default of 1000 ms is below the typical kubelet probe period (10 s default) and lameduck signalling is effectively invisible in a standard K8s deployment. Status should be Verified-Fixed-Partial — the wire-level mechanics are correct, the default is wrong.
- **REL-2-07** (tracing) — Round 7 closed as Verified-Fixed-Partial. ROUND8-OPS-06 escalates: subsequent rounds did not move the partial to complete. The library still has zero L7 callsites. Severity should be raised from "partial" to "high — no consumer".

## Findings *not* opened (and why)

- `/livez` always 200 — matches K8s convention; `ops-resolved.md`.
- ProbeRegistry single-atomic state machine — sufficient for pod-scoped K8s probes; `ops-resolved.md`.
- Per-listener bind tracking missing — equivalent to the simpler form we ship; `ops-resolved.md`.
- panic = abort decision — matches CODE-2-02 fix and Cloudflare's pattern; `ops-resolved.md`.
- BTF non-fatal load — matches L4 handoff cross-cutting item; `ops-resolved.md`.
- TaskTracker over JoinSet — matches CODE-2-03 recommendation; `ops-resolved.md`.

## Pattern: docs > code

The recurring theme across the high findings: docs assert capabilities the code does not implement.

- README claims FD-passing (OPS-01) — not implemented.
- README/RUNBOOK claim distributed tracing primitives (OPS-06) — library exists, no consumer.
- METRICS.md claims shutdown observability (OPS-03 implicit) — counter present, histogram absent.
- DEPLOYMENT.md systemd hardening (OPS-07) — defensible but below 2026 baseline.

The bash doc-lint gate (OPS-09) catches one class of doc-drift; it does not catch "the doc claims X works, the code does not implement X". A Rust-binary doc-lint (option in OPS-09 recommendation 3) is the structural fix.

## Pattern: defaults are conservative for the common case, wrong for streaming workloads

OPS-10 (drain budget) and OPS-11 (readiness settle) both ship defaults tuned for short-request HTTP. Operators of streaming workloads (gRPC, SSE, WebSocket) will hit both as deploy-time bugs. The fix is the same shape in both cases: document the per-workload default selection in `CONFIG.md`, add a per-listener override.

## Next steps for the reconcile / fix phases

- Phase C (reconcile): tag every finding NEW / MISSED / DISPUTE. ROUND8-OPS-03/-04 are MISSED-by-Round-7 (specific spec items the prior audit closed without verifying). ROUND8-OPS-06 is also MISSED (status note disclosed the gap; nobody re-opened it). Most others are NEW.
- Phase D (fix): all medium+ findings need plan-approval. OPS-01 (README rewrite) and OPS-03 (histogram wire-up) are the easiest one-liners; OPS-06 (tracing wire-in) is a multi-day refactor.
- Phase E (regression): rerun the round-7 gate matrix + add a "doc-claims-vs-code" gate as the structural fix for OPS-09.
