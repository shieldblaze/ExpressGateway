# Round-8 cross-review — by `div-l7`

Reviewer: `div-l7`. Round-8 cross-review pass over the L4 (`ROUND8-L4-*`) and OPS (`ROUND8-OPS-*`) findings. Posture: only non-trivial findings get more than one line; bar is honest divergence, not finding-count.

Verdict legend: **CONFIRM** / **CHALLENGE** / **ESCALATE-SEVERITY** / **CROSS-CUT**.

---

## L4 findings

### ROUND8-L4-01 — Backend-index 0 / zero-IP is a valid backend
**CONFIRM (high).** L7 corroboration: when XDP black-holes a flow by re-emitting toward `0.0.0.0`, the L7 view is "upstream pool entry healthy, connect succeeds, packets vanish" — same failure mode as the rustls / pingora "silent succeed" class. The `STAT_BACKEND_UNPOPULATED` counter is the correct lie-detector and parallels what L7 ought to learn from ROUND8-L4-10 (no observable signal == lesson unpaid-for).

### ROUND8-L4-02 — Pure LRU conntrack, no TCP state awareness
**CONFIRM (high).** **CROSS-CUT to L7**: when L4 evicts an established TCP conntrack entry mid-session, the L7 listener sees a *new* TCP accept with no prior state and runs the H1/H2 handshake from scratch. That is invisible from the L7 logs because the upstream LB-above retries onto the same VIP and we accept it as a fresh connection — only the *client* sees connection churn. So an LRU-thrash attack at L4 reads as "elevated conn/sec at L7" without any other signal. Worth adding to the L7-side observability story (no listener metric currently labels accepts as "new-flow vs reused-LRU").

### ROUND8-L4-03 — No SYN-flood new-flow-rate cap
**CONFIRM (high).** No L7 cross-cut beyond ROUND8-L4-02's overlap; the cap is L4-internal.

### ROUND8-L4-04 — Non-atomic backend-table updates (Unimog daisy-chain)
**CONFIRM (medium).** **CROSS-CUT**: during a backend hot-swap, an L7 upstream pool entry (`crates/lb-io/src/pool.rs`) keyed on `SocketAddr` continues to dispatch onto the *old* backend even after L4 has migrated. L7's pool eviction is health-driven, not L4-rebalance-driven — so during the inconsistency window, L7 actively *fights* the L4 swap. ROUND8-L7-resolved item R-L7-20 already notes the pool-key narrowness; this finding sharpens the requirement that pool eviction must observe L4 rebalance events, not just health probes.

### ROUND8-L4-05 — Drv→Skb attach fallback never runtime-probes XDP_TX
**CONFIRM (medium).** No L7 cross-cut — pure L4 driver-level lie-detector.

### ROUND8-L4-06 — `insert_acl_deny` accepts `prefix_len = 0`
**CONFIRM (high).** No L7 cross-cut — admission gate is correct as scoped.

### ROUND8-L4-07 — `BackendEntry::flags` is dead code; doc lies
**CONFIRM (medium).** No L7 cross-cut. Standard half-built-field hygiene.

### ROUND8-L4-08 — IPv4/IPv6 fragments silently forwarded
**CONFIRM (medium).** **CROSS-CUT**: L4 fragment mishandling can corrupt mid-stream payload bytes of an *active L7 TLS session*. The L7 side sees a rustls decryption failure on a previously-healthy connection and tears it down — an attacker who can spray fragmented UDP at the VIP could induce L7 TLS-decrypt errors on innocent neighbours sharing the conntrack 5-tuple namespace. Should escalate cross-cut visibility on the L7 observability side: add `lb_l7_tls_decrypt_error_total` with a `cause=l4_corruption_suspected` label?

### ROUND8-L4-09 — `ptr_at` bounds-check overflow shape
**CONFIRM (medium).** No L7 cross-cut. The author's framing (today no exploit; future-refactor footgun) is honest; arguably could be `low` today but `medium` covers the lesson-not-yet-paid-for stance.

### ROUND8-L4-10 — EBPF-2-07 "Verified-Fixed" but no verifier logs committed
**CONFIRM (high).** **CROSS-CUT — significant.** This is the L7-relevant audit-of-audit. The L7 side has parallel cases of "status notes Verified-Fixed-Partial with library-only fix and no L7 wire-in" (REL-2-07, REL-2-08 cardinality). ROUND8-OPS-06 is the L7-side twin: tracing library shipped, no callsite. Round-8 stance dictates that any `Verified-Fixed` grade without committed artefact (verifier-log, wire-in callsite, snapshot test) should be downgraded. Recommend the team-lead generalise the L4-10 ruling to a Round-8 policy: "no Verified-Fixed without committed artefact under `audit/round-8/closed/`".

### ROUND8-L4-11 — Loader does not enforce `/sys/fs/bpf` mount-type
**CONFIRM (medium).** No L7 cross-cut. Pure L4 startup hygiene.

### ROUND8-L4-12 — XDP attach does not use `BPF_F_REPLACE`
**CONFIRM (medium).** **CROSS-CUT to OPS-01**: this is *why* the FD-passing claim in README is doubly false — even if we built userspace FD passing today, our L4 attach path cannot atomically replace the old XDP program on the NIC. Re-attach failure mode A (EBUSY on the second deploy) is precisely the symptom an operator would hit when trying to do a hot reload. Worth linking explicitly in the OPS-01 fix plan.

---

## OPS findings

### ROUND8-OPS-01 — README claims FD-passing; not implemented
**CONFIRM (high).** L7 corroboration: I authored R-L7-14 (resolved/different-but-fine) noting the same hot-reload model gap. The doc-lint extension proposed in the recommendation block is the right shape and matches L7's REL-2-14 doc-drift lessons.

### ROUND8-OPS-02 — Drain has no jitter / randomisation
**CONFIRM (medium).** **CROSS-CUT**: at L7, every per-connection task observes the cancel in the same scheduler tick, *and* the per-protocol graceful-shutdown emit (H1 `Connection: close`, H2 `GOAWAY`, H3 `CONNECTION_CLOSE`) fires synchronously. The proposed Envoy-style `P(close) = elapsed / drain_timeout` is the right answer at both layers, but L7 must implement the jitter *inside* the protocol emit path (the cancel arm at line 2484 is the wrong place for H2-stream-level jitter; per-stream RST_STREAM(CANCEL) timing should also be jittered). Severity correct.

### ROUND8-OPS-03 — `shutdown_drain_seconds` histogram missing
**CONFIRM (medium).** No L7 cross-cut beyond the observability shape. Round-7 Verified-Fixed grade was wrong; supports the L4-10 policy escalation above.

### ROUND8-OPS-04 — TCP accept loop has no cancel arm
**ESCALATE-SEVERITY (medium → high).** L7-side rationale: the leaked-accepted-socket and per-IP counter-drift bug bites L7 directly. The L7 admission gate at `crates/lb-l7/src/h1_proxy.rs` increments `per_ip_inflight` *before* spawning the per-connection task. If `JoinHandle::abort()` cancels mid-spawn, the per-IP counter is incremented with no corresponding decrement. Over many drains this is a *monotonic accumulator* — eventually the per-IP cap rejects legitimate clients until pod restart. We have not paid for this because we drain rarely, but the bug shape is high-severity for any deployment with per-IP caps enabled (the default config). Recommend bumping to high in the team-lead arbitration.

### ROUND8-OPS-05 — `LabelBudget::check` only at startup
**CONFIRM (medium).** No L7 cross-cut beyond REL-2-08-Partial overlap.

### ROUND8-OPS-06 — Tracing propagation library exists, zero L7 callsites
**CONFIRM (high).** This is *the* representative L7 finding from the OPS pass — and the team should look at it in tandem with my own L7 findings, because every L7 finding's reproduction story would be vastly easier with per-request span IDs. The severity is correctly high. **CROSS-CUT — recommend the team-lead pair OPS-06 with L4-10 as a single Round-8 policy item: "library-only fix without consumer = Open, not Verified-Fixed-Partial".**

### ROUND8-OPS-07 — Systemd unit missing modern hardening
**CONFIRM (medium).** No L7 cross-cut. Hardening matches generic 2026-systemd baseline.

### ROUND8-OPS-08 — SBOM generated by manual-fallback
**CONFIRM (low).** No L7 cross-cut. Supply-chain-only.

### ROUND8-OPS-09 — `doc-lint.sh` is a ratchet, not a guard
**CONFIRM (medium).** **CROSS-CUT**: every L7 finding that calls out doc/code drift (R-L7-resolved entries citing REL-2-* status notes) is structurally vulnerable to the same "doc-lint catches only the bug it was written for" complaint. The recommendation to replace the bash script with a Rust integration test importing default consts is the correct shape.

### ROUND8-OPS-10 — Drain budget 10 s default below Pingora norm
**CONFIRM (medium).** **CROSS-CUT, important**: 10 s is materially insufficient for the L7 streaming workloads I already audited (H2 SSE, gRPC bidi, WS). My finding on body over-read marking the connection non-reusable (ROUND8-L7-10) is in the same operability cluster — once we mark a streaming connection non-reusable, the drain budget governs whether we can finish the in-flight stream. The two findings should be cross-referenced and the per-listener override implemented together.

### ROUND8-OPS-11 — `/readyz` flip-to-503 has no inflight grace
**CONFIRM (medium).** No L7 cross-cut beyond shared drain timeline. The readiness-flip-ack proposal is the right shape; mention it in any joint OPS-02 / OPS-10 fix plan.

### ROUND8-OPS-12 — Container image lacks RO rootfs + healthcheck + LABEL provenance
**CONFIRM (low).** No L7 cross-cut.

---

## Decisions for team-lead

1. **ESCALATE ROUND8-OPS-04 medium → high.** The accept-loop cancel-arm gap creates a monotonic per-IP counter drift bite under repeated drain — already-spawned admission gate state but no spawned task. Default config has per-IP caps enabled. Bug bites at second drain, not first; lesson-not-yet-paid-for shape. See cross-review entry for OPS-04 above.

2. **Generalise the L4-10 + OPS-06 policy.** Both findings are instances of "Verified-Fixed grade without committed artefact" (verifier log; L7 callsite). Recommend a Round-8 ruling that *no* finding gets Verified-Fixed without an artefact under `audit/round-8/closed/`, retroactively flagging REL-2-07, REL-2-08, EBPF-2-07 for re-verification. This is policy-level, not finding-level.

3. **Cross-link OPS-01 + L4-12.** The README "FD-passing" lie is doubly false because the L4 attach path can't `BPF_F_REPLACE` atomically either; fixing OPS-01 without L4-12 means the next attempt at FD-passing in code will fail at L4 attach. Bundle.

4. **Cross-link OPS-10 + ROUND8-L7-10.** Per-listener `drain_timeout_ms` override + body-overread-non-reusable interact in the streaming-listener case; treat them as one operability story.

No findings I'd CHALLENGE — all 24 (L4-01..12, OPS-01..12) read as honest divergences with severities defensible within ±1 step of the author's call.

End of L7 cross-review.
