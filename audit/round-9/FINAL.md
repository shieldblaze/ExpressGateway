# Round-9 — H3-green Terminal Verdict

Branch: `feature/h3-green` (base `prod-readiness/round-4`, strict) ·
HEAD `369df2ce` · Date: 2026-05-16 · Lead: owns this verdict.

## VERDICT: NO-GO

Reached via mission **exit condition (b)**: gates remain that are
blocked by work which cannot be completed in this environment or
within reachable scope. This is the honest binary verdict — not
"conditional", not faked. Substantial real, verified progress was
made; it is enumerated below alongside the exact remaining blockers.

---

## Final scorecard (mission gate list)

| Gate | Verdict | Evidence | Independently verified |
|------|---------|----------|------------------------|
| gate-07 cargo-deny | **PASS** | `cargo deny check` exit 0 (advisories/bans/licenses/sources ok), re-confirmed on final HEAD | yes (gate-runner ×2) |
| build + clippy + fmt | **PASS** | `cargo clippy --all-targets --all-features -D warnings` exit 0; `fmt --check` exit 0; workspace builds; no iteration-2 regression | yes |
| D-2 verifier — single kernel | **PASS** | cherry-picked `7af84128` build-xdp.sh produces a reproducible loadable BPF ELF; aya-parse + `elf_sections` 3/3 | yes (baseline + postcp) |
| D-1 native ENA attach | **PASS** *(documented constraint)* | live `mode=Drv`, no SKB fallback, kernel `ip -d link` bare `xdp`, non-zero STATS delta, RAII teardown fully restores ens5 | yes (verifier, reproduced live as root) |
| D-4 h2spec | **PASS** | harness-threading root cause fixed; independent external h2spec `147 tests, 146 passed, 1 skipped, 0 failed`; no waiver needed | yes (verifier, own listener) |
| D-5 docker + trivy | **PASS** | distroless image builds; 0 unwaived HIGH/CRITICAL; 1 documented `.trivyignore` waiver | yes (postcp) |
| **D-4 h3spec** (live run) | **NOT RUN** | h3spec binary present/works; live run against the QUIC/H3 listener never dispatched this run | — |
| **D-2 verifier — multi-kernel lvh (5.15/6.1/6.6)** | **BLOCKED (env)** | `quay.io/lvh-images/kernel-images` pull times out; no pinned digests; privileged-docker matrix unrunnable here | n/a |
| **D-3a chaos** | **FAIL — artifact absent** | no `chaos`/`soak` test target anywhere; no `lb-integration-tests` crate exists; previously mislabeled "infra-deferred" | n/a |
| **D-6 llvm-cov ≥80% (coverage-scope.md)** | **FAIL + BLOCKED** | per-crate proxy: 5 unwaived scoped modules <80% (h1_proxy 59%, h2_proxy 44%, quic `listener` 0%, quic `conn_actor` 76%, observability `metrics` 62%); authoritative `--workspace` llvm-cov exceeds 28 GB disk; branch% unmeasurable on pinned rustc 1.85 / cargo-llvm-cov 0.8.7 | n/a |

GO requires every gate full PASS with evidence. Four are not PASS.

---

## Real progress delivered this round (verified, committed, pushed)

1. **PROTO-2-12 H3 cross-bridge trailers** (`b9a2c274`) — the deferred
   Pillar-3b H3-to-upstream leg: `trailers` on
   `H3Request`/`H3UpstreamResponse`, RFC 9114 §4.1 post-DATA HEADERS
   emit/parse, hot-path flip in all four bridge calls, two new
   positive H3-leg tests. VERIFIED-PASS, no test weakened.
2. **D-1**: stub → genuine native `XDP_FLAGS_DRV_MODE` attach test on
   the ENA NIC, no SKB fallback, proven live (`3fd06339`).
   VERIFIED-PASS. Production constraint (no-frags ⇒ MTU≤3498 +
   channels≤max/2 on ENA) documented in DEPLOYMENT.md / RUNBOOK.md /
   `d1-native-xdp-constraint.md` (`cdb864b5`).
3. **D-4 h2spec**: real harness-threading root cause fixed
   (`450b6e80`); 0 failed conformance cases, independently corroborated.
4. **Observability gap**: 6 missing XDP stat-slot metric labels added
   in exact discriminant order; METRICS.md cardinality corrected
   (`450b6e80`). VERIFIED-PASS.
5. **Base remediation**: `c2bbfea6`→`7af84128` cherry-picked, flipping
   D-2-single and D-5 from FAIL to PASS.
6. Static gates re-confirmed regression-free on the final tree.

Every fix was independently re-verified by an author≠verifier agent;
no test was weakened, skipped, or deleted to pass any gate; no gate
was asserted PASS without verbatim command output.

---

## Exact remaining blockers and what each requires

### D-3a chaos — FAIL (missing implementation, scope > reachable)
There is no chaos or soak test in the tree and no
`lb-integration-tests` crate. `audit/round-8/regression/deferred-env.md`
framed D-3 as a wall-clock/load-infra deferral, but the test artifact
was never written. To clear this: author a chaos suite (fault
injection: backend kill/restart, latency, conn resets, partial writes)
**and** the load-generation harness; the soak half additionally needs
a separate ≥20k rps load box on the same L2 segment — environmentally
unavailable here. This is a multi-iteration workstream, not a reachable
single increment.

### D-6 coverage — FAIL + environment-hard-blocked
Two independent walls: (a) the authoritative measurement mandated by
`coverage-scope.md` is `cargo llvm-cov --workspace`, whose instrumented
build exceeds the 28 GB volume — it cannot be run here at all, and
`coverage-scope.md` explicitly forbids excluding the scoped modules;
(b) even the per-crate lower-bound proxy shows large hot-path modules
(`h1_proxy` 59%, `h2_proxy` 44%, quic `listener` 0%, `conn_actor` 76%,
observability `metrics` 62%) far below the 80% line gate and **not**
waived; (c) branch% (the ≥70% half of the gate) is unmeasurable on the
pinned rustc 1.85 / cargo-llvm-cov 0.8.7 (emits region%, not branch%).
The `docs/conformance/coverage.md` waiver table is also now stale
(claims `observability/lib.rs` 100%; actual 62% post-iteration-2).
Clearing this requires substantial new unit tests for several hot-path
modules **and** either a larger-disk runner or a toolchain bump —
partly environmental, partly large-scope.

### D-2 multi-kernel lvh matrix — BLOCKED (environment)
`quay.io/lvh-images/kernel-images:{5.15,6.1,6.6}` not pullable
(registry timeout); no pinned digests committed. The single-kernel
verifier PASS stands; the production multi-LTS matrix cannot be run
without registry access or pre-seeded images.

### D-4 h3spec — NOT RUN
Reachable in principle (h3spec binary works; needs the QUIC/H3
listener live with TLS), but not dispatched this run. Even a PASS here
would not change the verdict while D-3a/D-6 remain.

---

## Why NO-GO is the correct ending (not a loop continuation)

The loop made real progress every iteration until it reached gates
whose closure is either (i) absent implementation of scope larger than
a reachable increment (D-3a chaos suite + load infra) or (ii)
environment-hard-blocked such that a truthful PASS cannot be asserted
here regardless of effort (D-6 authoritative `--workspace` llvm-cov vs
28 GB disk; D-2 lvh registry). Continuing to loop would either spin
without progress or pressure a faked/weakened pass — both explicitly
forbidden by the mission. NO-GO with these specifics is the honest,
correct terminal state.

## Recommended path to a future GO (out of scope for this environment)
- Larger-disk CI runner (≥80 GB) for the authoritative `cargo llvm-cov
  --workspace`; toolchain with branch-coverage support.
- Author the chaos/soak suite + load harness; provision the soak load
  box.
- Add unit tests lifting `h1_proxy`/`h2_proxy`/quic `listener`/
  `conn_actor`/observability `metrics` ≥80%; refresh the stale
  `coverage.md` waiver table.
- Registry access or pre-seeded lvh images for the D-2 multi-kernel
  matrix; then commit the `.log.committed` baselines.
- Rebuild `lb_xdp` with XDP multi-buffer/frags to remove the D-1
  jumbo-MTU deployment constraint.
- One increment to run h3spec live against the QUIC/H3 listener.
