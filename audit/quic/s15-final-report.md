# SESSION 15 — FINAL REPORT (native QUIC proxy, Mode A passthrough)

> Program: a native QUIC proxy for ExpressGateway. **Mode A = passthrough**
> (the LB routes QUIC by connection-ID without terminating TLS — it never
> decrypts). Increments A0 (design) → A1 (public-header parser) → A2
> (passthrough datapath) → A3 (threat-defences + observability) → A4
> (this promote). Mode B (raw stream/datagram proxy) is **S16**.

## Outcome

**SESSION 15 COMPLETE** — Mode A passthrough datapath built, verified, and
promoted to main. The matrix/H-to-H work (S6–S14) is unaffected.

## What landed (by increment)

- **A1** — `lb_quic::public_header::parse_public_header` (SHARED-1), proptest-budgeted.
- **A2** — `lb_quic::passthrough` datapath: public-header parse → Maglev
  backend pick (hash of client DCID) → per-flow `connect()`-ed backend
  socket (kernel anti-spoof) → bidirectional pump; LRU dispatch table
  bounded at 2×cap; `FlowEntry` holds NO key material (compile-time
  destructuring audit). Stateless Retry minting (Initial-flood defence).
  Resume (post-ENOSPC) closed gate (v) R3 ×3 + the `mint_retry` trusted-
  network escape + cov baseline.
- **A3** — threat-model defences + observability:
  - `lb_observability::PassthroughMetrics` (new module, mirrors XdpMetrics):
    `quic_passthrough_flows` (gauge = dispatch-table size, `.set(table.len())`),
    `_flows_evicted_total`, `_retry_minted_total`, `_retry_rejected_total`,
    `_header_parse_errors_total`, `_backend_socket_errors_total`. Threaded
    via `PassthroughParams.metrics: Option<_>` (None = no-op in tests),
    registered in `spawn_passthrough`.
  - Audit-log throttle: `audit_allow(&AtomicU64, now_ms, window)` CAS gate
    → one `warn!(event="audit/source_binding_violation")` and one
    `audit/quic_passthrough_cap_hit` per `audit_throttle_window` per
    category (was a knob with no enforcement before).
  - `min_client_dcid_len` floor (already in the A2 tree; A3 added its test).
  - 9 table-driven unit tests lifting passthrough.rs coverage 75.91% → 91.87%.

## Verify-bar (A3, design §A3) — ALL PASS

| Gate | Result |
|---|---|
| 1. Spoofed-source-IP e2e (drop + `audit/source_binding_violation`) | PASS |
| 2. Audit-throttle saturation (1 line/window, not per-event) | PASS |
| 3. `quic_passthrough_*` metrics increment correctly | PASS |
| 4. llvm-cov ≥80% on passthrough.rs | **PASS — 91.87% (972/1058)**, verifier-confirmed |
| 5. ×3 workspace gate + clippy + fmt | PASS — RUN2/RUN3 **1349/0/18**, clippy+fmt clean |

Gate 5 RUN-1 had one failure (`fcap1_h2_over_cap_upload_yields_413`) — the
known pre-existing F-CAP-1 saturation flake ([[fcap1-overcap-arm-backpressure-masked]]),
passed 2/3, outside A3's change surface. Environmental (R2).

Author≠verifier upheld: builder-1 authored impl; verifier independently
authored gates 1-3, re-measured cov (91.87%, matched), ran clippy/fmt.
Evidence: `audit/quic/s15-a3-verify-evidence.md`.

## Resume note (this session)

A prior S15 session lost in-flight work to ENOSPC. This session: recovered
+ pushed the `mint_retry` work (a8668db6), stood up a build-safe +
llvm-cov-aware disk-guard loop, closed the disk-blocked A2 gate (v) ×3 +
cov, then built + verified A3. Two near-ENOSPC events were caught and
reclaimed live (see [[disk-cleanup-loop-must-not-race-builds]]).
Evidence: `audit/quic/s15-resume-evidence.md`.

## Carry-forwards (open, dedicated work)

- **CF-S15-PASSTHROUGH-FEATURE-GATING-DEEP** — A2 gate (ii) NEVER-DECRYPTED
  LINKAGE: gate `lb-io::quic_pool` + lb-l7 H1/H2/H3 `QuicUpstreamPool`
  consumers + lb's `build_h3_upstream_pool` behind a `quic-upstream`
  feature so the passthrough-only binary links zero quiche termination
  symbols. Multi-crate refactor (~42 use sites). NOT in S15 close.
- **CF-S15-PASSTHROUGH-RETRY-ODCID** — LB-mints-Retry hides the client
  ODCID from the backend (transport-param mismatch). Mitigated by the
  `mint_retry=false` trusted-network escape; the production fix is a QUIC
  PROXY-protocol-analogue sidecar (token-embedded ODCID). S15.x / S16.
- **Per-IP Retry rate-limit** — design defers to v1.1 (ticket only).
- **CF-S15-A2-LLVMCOV** — CLOSED (passthrough.rs now 91.87%).

## S16 handoff

See `audit/quic/s16-handoff.md`. Headline: Mode B (raw stream/datagram
proxy) reusing SHARED-1 `parse_public_header`, SHARED-2 `UdpDataplane`,
and `build_retry_packet`.
