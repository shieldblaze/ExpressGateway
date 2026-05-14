# Round 8 Phase C — Recheck (verify)

Sample size: 17 spot-checks across 5 prior areas (target ≥20% of 70 =
14). For each, locate the cited commit, `git show <sha> --stat` to
confirm the diff content, and verify the diff matches the
finding's *recommendation* (not just that something landed).

Verdicts:
- **STILL-VERIFIED** — the cited commit closes the original recommendation.
- **PARTIAL** — the commit closes part of the recommendation; the prior
  audit already disclosed `Verified-Fixed-Partial` or the partial gap
  is undisclosed.
- **FALSE-VERIFIED** — the commit does not close the recommendation;
  the audit register status overstates reality.

---

## sec (5 samples)

### SEC-2-01 — SmuggleDetector hot-path wire-up
- Cited commits: `e36b50f` + `0c7e16b` + `e00e85a` + `5e7938f`
- Diff content: HooksBundle trait + production impl + per-request `check_all_mode` call site in H1Proxy::handle + 13 proof tests in smuggle_strict_te. Detector fires on every request in lb-l7 hot path; on Err returns 400.
- Verdict: **STILL-VERIFIED**

### SEC-2-04 — Per-IP / per-listener concurrent cap
- Cited commits: `e36b50f` + `8e048c0` + `4001791`
- Diff content: ConnGate + HooksBundle wired at accept site BEFORE listener inflight semaphore (verified at `crates/lb/src/main.rs:2225-2270` post-edit). `conn_gate.rs` proof test exists with 10 tests covering listener cap + per-IP cap + GC + permit Drop semantics. AcqRel/Acquire on success CAS confirmed per SEC-2-16 mandate.
- Verdict: **STILL-VERIFIED**

### SEC-2-05 — 0-RTT replay window LRU
- Cited commit: `eeae98a`
- Diff: lb-security/src/zero_rtt.rs +266/-39, true LRU with doubly-linked list + slab arena + HashMap<digest, slot>. 9 proof tests including `replay_hit_promotes_to_mru` (FIFO regression guard), `fills_and_evicts_under_unique_token_spray`, `arena_reuses_freed_slots`. HMAC-SHA256 digest preserved.
- Verdict: **STILL-VERIFIED**

### SEC-2-06 — Admin auth + bind-loopback gate
- Cited commits: `baa72ca` + `9484544`
- Diff: lb-security/admin_auth.rs (AdminTokenHash + AdminAuthGate + validate_bind) lands in baa72ca. 9484544 wires `validate_bind` in `lb/src/main.rs` BEFORE admin HTTP bind + `serve_with_auth` bearer enforcement on `/metrics` (probes stay anon). 14 unit + integration tests including non-loopback refusal.
- Verdict: **STILL-VERIFIED**

### SEC-2-11 — XDP CAP_SYS_ADMIN fallback
- Cited commit: `e44117d`
- Diff: `probe_caps_with` closure-based probe with CAP_BPF→CAP_SYS_ADMIN fallback; `tests/xdp_cap_probe.rs` with 7-test mock matrix. Cross-area verification by `rel` round-5 + cargo test pass on round-4.
- Verdict: **STILL-VERIFIED**

## code (4 samples)

### CODE-2-02 — panic = "abort" + tracing-backed hook
- Cited commits: `120e4fa` + `b6aeea5`
- Diff: `panic = "abort"` in `[profile.release]`; `std::panic::set_hook` installed in `main.rs:97` before any spawn; emits structured `tracing::error!(panic=true, message, location, backtrace)` and bumps `panic_total`. Dev/test deliberately keep unwind for proptest/loom. `init_panic_hook` installed before any spawn.
- Verdict: **STILL-VERIFIED**

### CODE-2-03 — Graceful drain replaces JoinHandle::abort()
- Cited commits: `9ff2b9b` + `fc050b0` + `bca4285`
- Diff: Shutdown primitive in lb-core, accept-site plumbing in lb/src/main.rs across L7/QUIC/IO, per-conn task tracker + biased select! cancel arm + `shutdown_aborted_connections_total` counter.
- **Round-8 audit finding**: The status note claims "listener accept loop" is migrated to `shutdown.tracker().spawn(...)`. That is true for the spawn — but the **accept loop body itself** at `crates/lb/src/main.rs:2180` still does `listener.accept().await` with no `tokio::select!` cancel arm, and the drain at line 1942-1944 still uses `JoinHandle::abort()`. Per-connection cancellation is correct; accept-loop cancel is missing. (ROUND8-OPS-04 captures this.)
- Verdict: **PARTIAL** (per-conn correct; listener accept loop not yet cooperatively cancellable; prior audit overclaimed scope)

### CODE-2-09 — Non-blocking upstream connect
- Cited commits: `f07cf44` + `fc42d60`
- Diff: `TcpPool::acquire_async` at pool.rs:210 uses `tokio::net::TcpStream::connect` under `PoolConfig::connect_timeout`. Every upstream dial site migrated (lb-l7 h1/h2/grpc + lb-io http2_pool + lb-quic h3_bridge). Static no_spawn_blocking_in_pool_dial_path test + acquire_async_timeout_fires test. `connect_timeout-from-runtime` plumb is the documented 1-line follow-up (defaults to 5_000 ms which already matches the config default, so behaviour unchanged).
- Verdict: **STILL-VERIFIED** (the deferred 1-line plumb is documented; behaviour matches default; not a false-verified)

### CODE-2-14 — Backend counter sync (lb-balancer ↔ lb-core)
- Cited commits: `e3ac961` + `7399044`
- Diff: lb-balancer + lb-core dep; Backend.state field; with_state constructor + sync_from_state() with three Acquire loads. 7399044 lands the missing `tests/balancer_counter_sync.rs` JoinSet race-test that Round-5 ebpf verifier flagged absent. Loom model at `loom_atomic_counter.rs` covers publication ordering.
- Verdict: **STILL-VERIFIED** (Round-5 push-back closure properly addressed)

## ebpf (3 samples)

### EBPF-2-03 — CONNTRACK → LRU_HASH
- Cited commit: `c009219`
- Diff: aya_ebpf LruHashMap import; CONNTRACK + CONNTRACK_V6 typed as LruHashMap with identical capacities; userspace HashMap accessor unchanged. Test pair `flood_does_not_oom_userspace_simulator` (always-on CI signal) + `lru_evicts_oldest_under_flood` (#[ignore]'d, kernel-side).
- **Round-8 audit comment**: ROUND8-L4-02 captures that LRU solves ENOMEM-on-fill but **not** the TCP-state-aware pruning gap (Cilium conntrack.h). That is an unaddressed scope, not a regression on EBPF-2-03.
- Verdict: **STILL-VERIFIED** (within original recommendation scope; new finding extends scope)

### EBPF-2-04 — XDP attach probe ladder + xdp_mode knob
- Cited commit: `75d4740`
- Diff: Replaces hard-coded XdpMode::Skb attach with Drv-then-Skb probe ladder; new `RuntimeConfig.xdp_mode` knob defaulting to "auto"; integration test + metric reporting.
- **Round-8 audit comment**: ROUND8-L4-05 captures the silent-drop class (aya #1193, MLX5/CX6 firmware fail) — fallback trigger is attach-syscall errors, not post-attach packet probing. Out of original EBPF-2-04 recommendation scope.
- Verdict: **STILL-VERIFIED** (recommendation scope met; ROUND8-L4-05 is a scope-extension finding)

### EBPF-2-07 — Verifier-log matrix capture
- Cited commit: `ffde98c`
- Diff: `scripts/verify-xdp.sh` (+109) and `audit/ebpf/verifier-logs/README.md` (+30). **NO `.log.committed` baselines**. The diff-gate at `scripts/verify-xdp.sh:111` checks `if [ -f "${OUT_LOG}.committed" ]; then diff ...` — the `.committed` file does not exist; the conditional is always false; the gate is a permanent no-op. `audit/unsafe-justifications.md:109` claims the gate is live in CI — false.
- Verdict: **FALSE-VERIFIED** (script harness shipped; baselines did not; gate does not fire; this is the canonical audit-of-audit dispute — see `audit/round-8/disputes.md`)

## rel (3 samples)

### REL-2-02 — Drain ordering + shutdown_drain_seconds histogram
- Cited commits: `1f7ab4b` + `fc050b0` + `82551dc` + `33edd13` + `task-38`
- Diff: drain at main.rs:1699-1759 with set_draining → 1s settle → cancel → listener.abort → quic.shutdown(2s) → shutdown.drain(10s). H2 graceful_shutdown emits two-step GOAWAY. H1 hyper http1 graceful_shutdown wired. Task-38 closed the three drain test gaps. All three drain integration tests pass.
- **Round-8 audit finding**: The REL-2-02 recommendation at line 134 explicitly required *two* metrics: `shutdown_drain_seconds` histogram AND `shutdown_aborted_connections_total` counter. The counter shipped (`crates/lb/src/main.rs:1974`); the histogram did NOT (`grep shutdown_drain_seconds crates/` returns zero hits in source code — only documentation/comment references). ROUND8-OPS-03 captures this.
- Verdict: **PARTIAL** (drain ordering correct; one of two required metrics absent; prior audit overclaimed)

### REL-2-04 — `/livez+/readyz+/startupz` split
- Cited commit: `7108d9e`
- Diff: ProbeRegistry AtomicU8 state machine over {Starting, Ready, Draining}; /livez stays 200 across Ready+Draining; /readyz flips to 503 on `set_draining()`; /startupz transitions once every listener is bound. JSON contract test updated.
- **Round-8 audit comment**: ROUND8-OPS-11 captures that `readiness_settle_ms=1000` is below kubelet default probe period (10 s); operational gap, not a regression on REL-2-04 recommendation.
- Verdict: **STILL-VERIFIED** (within original scope; OPS-11 is a default-tuning gap)

### REL-2-14 — Binary name mismatch
- Cited commit: `f2bf64c`
- Diff: `grep -n '/usr/local/bin/lb\b\|target/release/lb\b' DEPLOYMENT.md RUNBOOK.md` returns zero hits; every path replaced with `expressgateway`. doc-lint.sh gate added with STALE_PATTERNS for the legacy binary names.
- **Round-8 audit comment**: ROUND8-OPS-01 + ROUND8-OPS-09 capture the doc-lint *philosophy* gap (only the binary-name bug class is patrolled; FD-passing / drain values / capability claims are not). Not a regression on REL-2-14 recommendation as written.
- Verdict: **STILL-VERIFIED** (within original scope; OPS-01/-09 are scope extensions)

## proto (2 samples)

### PROTO-2-01 — H2 `:authority` ↔ Host agreement
- Cited commit: `132fc72` (audit-sha 3586367)
- Diff: `check_authority_host_agreement` runs in H2Proxy::handle BEFORE hop-by-hop strip; case-insensitive + default-port latitude + IPv6-aware; returns 400 on mismatch with belt-and-braces guard in H2ToH1Bridge::bridge_request. Proof tests pass.
- Verdict: **STILL-VERIFIED**

### PROTO-2-02 — QUIC ALPN = `h3`
- Cited commits: `c941b28` + `81079fb` (audit-sha 3586367)
- Diff: `set_application_protos` pinned `&[b"h3", b"h3-29"]` at both call sites; static guard test `production_alpn_constant_is_h3` + handshake test `server_rejects_unknown_alpn`. No `lb-quic` ALPN in production.
- Verdict: **STILL-VERIFIED**

---

## Recheck summary

| Verdict | Count |
|---|---|
| STILL-VERIFIED | 13 |
| PARTIAL        |  3 (CODE-2-03 listener-cancel, REL-2-02 histogram, EBPF-2-04 silent-drop — last two were scope-extension; only REL-2-02 is undisclosed) |
| FALSE-VERIFIED |  1 (EBPF-2-07 — no baselines committed) |

Effective false-verified rate **for the disputed contract** (the
"Verified-Fixed" status claims the recommendation closure):
- CODE-2-03 listener accept loop: status note says all 5 spawn sites
  migrated; the accept loop body itself still uses
  `JoinHandle::abort()` and no `select!` cancel arm. **Undisclosed PARTIAL.**
- REL-2-02 histogram: the two-metric spec is half-shipped.
  **Undisclosed PARTIAL.**
- EBPF-2-07 verifier-log diff gate: the gate is a permanent no-op.
  **FALSE-VERIFIED.**

That's 2 undisclosed PARTIALs + 1 FALSE-VERIFIED out of 17 spot-checks
= **17.6% effective false-or-partial rate**.

## Escalation trigger

Rule: ≥3 FALSE-VERIFIED out of 15 (20%) → must recheck ALL prior
Verified-Fixed.

We have:
- 1 FALSE-VERIFIED outright (EBPF-2-07).
- 2 undisclosed PARTIALs that the prior register filed as
  `Verified-Fixed` (REL-2-02 histogram half, CODE-2-03 listener-loop half).

Per the strict rule wording (FALSE-VERIFIED count), we are at **1/17 =
5.9%**, below the 20% threshold. Per the spirit (the prior register
overclaims completion), we are at **3/17 = 17.6%**, still below the
trigger but the discovered pattern is concerning enough to flag.

**Recommendation to lead** (also see final summary):
- Escalation rule does NOT trigger by literal count of FALSE-VERIFIED.
- However the pattern is "the audit shipped the obvious half; the
  follow-up half was filed under the same finding ID as
  Verified-Fixed". Phase D should bundle these into the doc-lint
  audit-of-audit gate (ROUND8-L4-10 / ROUND8-OPS-09).
- Do not loop back to a deeper Round 1-7 redo; instead, in Phase D,
  the 14 MISSED items plus the 3 partial/dispute reclassifications
  should be the primary plan-owner work.
