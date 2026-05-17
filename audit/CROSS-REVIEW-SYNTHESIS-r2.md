# Round 2 — Cross-Review Synthesis & Severity Arbitration (team-lead)

70 findings landed across the five areas. The cross-review pass produced
a small set of severity escalations and one downgrade; all are
non-controversial and concurred by ≥2 teammates. This file records the
final severity table, applies the arbitrations, and pre-stages the
Round-3 plan-owner assignment.

## A. Severity arbitrations applied (all backed by ≥2 teammates)

| Finding | From | To | Concurring teammates | Rationale |
|---|---|---|---|---|
| **CODE-2-02** `panic = "abort"` not set | high | **critical** | code (auth) + proto + rel via cross-cuts | Default `unwind` across `unsafe` boundaries leaves a partially-broken process running; that is worse than abort for a network proxy with 71 `unsafe` sites. |
| **REL-2-15** No panic hook + `unwind` | high | **critical** | rel (auth) + code + proto | Same root cause as CODE-2-02; merge for Round-3 planning under one fix. |
| **PROTO-2-09** Silent fall-through to PlainTcp | medium | **high** | proto (auth) + code + sec | Operator-error attack surface: typo in `protocol = "https"` silently produces a plaintext TCP forwarder. Not an RFC issue; a safe-defaults issue. |
| **PROTO-2-15** SNI ↔ Host/`:authority` not enforced | medium | **high** | proto (auth) + sec | RFC 6066 §3 + RFC 9110 §7.4 + RFC 9113 §8.3.1 + RFC 9114 §4.3 — four-layer authority-coherence violation; combined with smuggling becomes a routing-confusion primitive. |
| **SEC-2-12** BPF ELF license / loader license-string | medium | **low** | sec (auth, self-downgrade) + ebpf | ebpf confirmed aya-obj 0.2.1 defaults to `"GPL"`; today's loads succeed. The fragility is real but not currently exploitable; keep as a low-severity hygiene finding for the ELF section. |

## B. Unified findings register (post-arbitration)

| ID | Severity | Owner | Title (one line) | Joint with |
|---|---|---|---|---|
| **Critical (11) — every one is blocking-for-prod** | | | | |
| CODE-2-01 | critical | code | lb-l7 has no lb-security dep; detectors are dead code | SEC-2-01/03/04/10, PROTO-2-10 |
| CODE-2-02 / REL-2-15 | critical | code | `panic = "abort"` + `set_hook` | — (merged) |
| CODE-2-03 | critical | code | TCP/H1/H1s accept loop: no graceful drain on SIGTERM | REL-2-02, PROTO-2-11 |
| CODE-2-05 / REL-2-09 | critical | code | Unbounded `tokio::spawn` per accept | (rel owns metric) |
| CODE-2-06 / REL-2-10 | critical | code | EMFILE/ENFILE tight-loop on accept | (rel owns metric) |
| REL-2-02 | critical | rel | SIGTERM drain ordering + 10 s default | CODE-2-03, PROTO-2-11 |
| REL-2-03 | critical | rel | TLS cert rotation actually wired | — |
| PROTO-2-02 | critical | proto | `LB_QUIC_ALPN = b"h3"` (not `b"lb-quic"`) | — |
| **High (23)** | | | | |
| SEC-2-01 | high | sec | SmuggleDetector wired into H1/H2 hot path | CODE-2-01, PROTO-2-10 |
| SEC-2-02 / EBPF-2-03 | high | ebpf | CONNTRACK → `BPF_MAP_TYPE_LRU_HASH` | REL-2-12 |
| SEC-2-04 | high | sec | Per-IP + per-listener concurrent-conn cap | CODE-2-05 |
| CODE-2-04 | high | code | Atomic ordering audit (50 Relaxed sites) | SEC-2-16 (input list) |
| CODE-2-07 / EBPF-2-09 | high | code | Pod constructor zero-init invariant | SEC-2-09 |
| CODE-2-08 | high | code | lb-quic per-CID DashMap leak on actor panic | — |
| CODE-2-09 / REL-2-11 | high | code | Remove `spawn_blocking` for connect | — |
| CODE-2-11 | high | code | Bootstrap proptest/loom/miri (workspace) | — |
| EBPF-2-01 | high | ebpf | BPF ELF: add `license` + `BTF` sections | — |
| EBPF-2-04 | high | ebpf | XDP attach: Native → Drv → SKB probe | — |
| EBPF-2-05 | high | ebpf | Map pinning under `/sys/fs/bpf/expressgateway/` | (sec on perms) |
| REL-2-01 | high | rel | Doc/code drift remediation (umbrella) | — |
| REL-2-04 | high | rel | `/livez` + `/readyz` + `/startupz` split | — |
| REL-2-05 | high | rel | Wire HealthChecker + ConfigManager | CODE-2-13 |
| REL-2-07 | high | rel | Distributed tracing + traceparent | — |
| REL-2-12 | high | rel | CONNTRACK saturation metric + alert | EBPF-2-08, REL-2-13 |
| PROTO-2-01 | high | proto | H2 reject when `:authority` ≠ `Host` (RFC 9113 §8.3.1) | — |
| PROTO-2-09 | high | proto | Hard-error on unknown `protocol = …` | — |
| PROTO-2-10 | high | proto | Wire SmuggleDetector or document hyper-only matrix | CODE-2-01, SEC-2-01 |
| PROTO-2-11 | high | proto | H2 GOAWAY + H3 CONNECTION_CLOSE on drain | REL-2-02, CODE-2-03 |
| PROTO-2-15 | high | proto | SNI ↔ Host/`:authority` disagreement reject | SEC-2-06 |
| **Medium (20)** | | | | |
| SEC-2-03 | medium | sec | Slowloris / SlowPost detector wiring (+ TLS handshake) | SEC-2-10 |
| SEC-2-05 | medium | sec | 0-RTT replay window resizing under unique-token spray | — |
| SEC-2-06 | medium | sec | Admin HTTP authn + bind-to-loopback default | REL-2-04 |
| SEC-2-07 | medium | sec | CI: `cargo audit -D warnings`, `cargo deny`, `cargo geiger` | — |
| SEC-2-10 | medium | sec | TLS-handshake timeout on `acceptor.accept()` | SEC-2-03 |
| SEC-2-14 | medium | sec | Wire `lb-compression::Decompressor` or remove crate | — |
| CODE-2-13 | medium | code | Wire `lb-controlplane` + `lb-health` or remove from `[dependencies]` | REL-2-05 |
| CODE-2-14 | medium | code | Single source of truth for backend counters | — |
| EBPF-2-02 | medium | ebpf | Add `#[link_section = "license"]` static in eBPF crate | EBPF-2-01 |
| EBPF-2-07 | medium | ebpf | Verifier-log matrix: 5.15 / 6.1 / 6.6 in CI | — |
| EBPF-2-08 | medium | ebpf | Export per-CPU STATS map to Prometheus | REL-2-13 |
| EBPF-2-09 | medium | code | (Pod userspace fix — merged into CODE-2-07) | (closed by CODE-2-07) |
| REL-2-06 | medium | rel | Structured JSON log output | — |
| REL-2-08 | medium | rel | Per-listener / per-backend RED labels | — |
| REL-2-13 | medium | rel | Export STATS map metrics (paired with EBPF-2-08) | EBPF-2-08 |
| PROTO-2-03 | medium | proto | 1xx / 100-Continue / 103 forwarding policy + test | — |
| PROTO-2-04 | medium | proto | Real Autobahn fuzzingclient CI run | — |
| PROTO-2-05 | medium | proto | h3spec / h3i / quic-tracker harness | — |
| PROTO-2-12 | medium | proto | Trailer pass-through tests across H1↔H2/H3 | — |
| PROTO-2-14 | medium | proto | `tls13_only` config switch (TLS 1.2 still enabled) | SEC-2-08 |

### Low / Info (16) — eligible for batch fix or deferral

These will be batched per-area or moved to `audit/deferred.md` with
justification in Round 3 — the Round-3 cadence is medium+ only.

- SEC: SEC-2-08, SEC-2-09 (closed by CODE-2-07), SEC-2-11, SEC-2-12, SEC-2-13 (info), SEC-2-15 (info), SEC-2-16 (info)
- CODE: CODE-2-10 (info — regression test only), CODE-2-12, CODE-2-15
- EBPF: EBPF-2-06 (regression test only)
- REL: REL-2-14
- PROTO: PROTO-2-06, PROTO-2-07, PROTO-2-08, PROTO-2-13

## C. Per-area Round-3 plan workload

| Area | Crit | High | Med | Low/Info | Plans to author (med+) |
|---|---|---|---|---|---|
| sec | 0 | 3 | 6 | 7 | 9 |
| code | 5 | 5 | 2 | 4 | 12 |
| ebpf | 0 | 4 | 4 (one closed) | 1 | 7 |
| rel | 4 | 6 | 3 | 1 | 13 |
| proto | 1 | 5 | 5 | 4 | 11 |
| **Totals** | **11** | **23** | **20** | **17** | **52** |

(11 critical + 23 high + 20 medium − 2 merged duplicates that fold into
one fix plan = **~52 plans** to author in Round 3, distributed as
above.)

## D. File-ownership map for Round 4 fix implementation

To prevent edit conflicts under the "no two teammates touch the same
file in the same round" rule, the lead-level ownership map is:

| Source file / area | Round-4 owner | Findings landing here |
|---|---|---|
| `Cargo.toml` (workspace) | code | CODE-2-02 (panic=abort), CODE-2-12, CODE-2-13, CODE-2-15 |
| `crates/lb/src/main.rs` (binary glue) | code | CODE-2-01/03/05/06, REL-2-05 (wire-up), REL-2-09/10 (metric calls), CODE-2-09, REL-2-14 |
| `crates/lb-l7/Cargo.toml` + `src/*.rs` | sec | SEC-2-01/03/04/10, PROTO-2-10 (wiring) |
| `crates/lb-l7/src/h2_proxy.rs` + bridges | proto | PROTO-2-01, PROTO-2-07, PROTO-2-08, PROTO-2-11, PROTO-2-15 |
| `crates/lb-l4-xdp/ebpf/src/main.rs` | ebpf | EBPF-2-02 (link_section), EBPF-2-03 (LRU) |
| `crates/lb-l4-xdp/src/loader.rs` | ebpf | EBPF-2-01 (ELF), EBPF-2-05 (pinning), EBPF-2-08 |
| `crates/lb/src/xdp.rs` | ebpf | EBPF-2-04 (probe), EBPF-2-06 (test) |
| `crates/lb-observability/*` | rel | REL-2-04/06/07/08/12/13, EBPF-2-08 (export) |
| `crates/lb-security/src/ticket.rs` | rel | REL-2-03 (cert rotation) |
| `crates/lb-quic/src/router.rs` | code | CODE-2-08 |
| Atomics across crates | code | CODE-2-04 — multi-file, but code serializes |
| `crates/lb-l7/src/listener.rs` | proto | PROTO-2-09 |
| `crates/lb-quic/src/config.rs` (ALPN) | proto | PROTO-2-02 |
| Test scaffolding under `tests/` | (varies) | Each owner writes their own proof tests; no sharing |
| `RUNBOOK.md` + `DEPLOYMENT.md` + `METRICS.md` | rel | REL-2-01 umbrella; **lead may edit these too** |

Cross-area conflicts:
- `crates/lb/src/main.rs` is touched by code (CODE-2-01/03/05/06/09), rel
  (REL-2-05/09/10), and indirectly proto (via wire-up of PROTO-2-02 /
  PROTO-2-09 since the bind+protocol selection lives there). The lead
  serialises: **code lands first**, then rel layers metrics on the new
  scaffolding, then proto edits the bind-protocol matching.
- `lb-observability` is rel's house; ebpf hands rel a map-reader API
  patch via a separate file `stats_export.rs` to avoid contention.

## E. Genuine cross-area decisions reserved for the lead

1. **PROTO-2-07 hop-by-hop**: proto recommends `StrippedRequest`
   newtype (option c, credit code). **APPROVED** for Round-3 plan.
2. **PROTO-2-14 TLS 1.2**: keep as medium for now; do **not** make
   tls13_only the default until proto + sec produce evidence that no
   relevant upstream / client requires TLS 1.2. Round-3 plan should
   add the knob but default to current behaviour.
3. **CODE-2-04 atomic ordering**: SEC-2-16 provided the
   security-relevant ordering requirements. code applies them site-by-
   site; lead does NOT require a separate plan per atomic — one plan
   covering the entire workspace audit is sufficient.
4. **CODE-2-09 + REL-2-11 spawn_blocking**: code's recommended
   alternative is `tokio::net::TcpStream::connect` (non-blocking).
   **APPROVED**.
5. **EBPF-2-09 / CODE-2-07 Pod**: merged into a single plan under code.
6. **CODE-2-13 + REL-2-05** wiring of `lb-controlplane` + `lb-health`:
   this is a feature gap, not a bug fix. **DECISION**: the audit's
   scope is "ready for production"; if the control-plane and active
   health-checker are not yet wired, they cannot be required for
   sign-off. Lead splits this into:
     - **Required for prod**: passive health (already in code) +
       sane file-backed control-plane that reads the existing TOML.
     - **Deferred**: distributed control-plane backends.
   Both `code` and `rel` write Round-3 plans against the required slice.

## F. Round 3 entry checklist

- STATE flips to `ROUND_PLANNING`.
- Every medium+ finding gets a fix plan, written by its owner under
  `audit/<area>/plans/<ID>.md`.
- Plan format (lead-mandated):
  ```
  # Plan for <ID> — <title>
  Finding-ref:    <ID> (severity, status)
  Files touched: <list>
  Approach:       <≤500 words>
  Proof:          <test name, benchmark, or invariant that proves the fix>
  Risk / blast radius: <what could break>
  Owner:          <name>
  Lead-approval-required: yes (this is a source-touching change)
  ```
- Lead reviews each plan and either APPROVES (records SHA-ready
  green-light) or REJECTS with concrete change.
- Low/info batched plans land in `audit/<area>/plans/batch-low.md`.
- Deferred items go to `audit/deferred.md` with a one-line
  justification and the lead's pre-emptive ack (you, the user, sign
  off at FINAL).
