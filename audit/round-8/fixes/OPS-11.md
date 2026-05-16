# Fix plan — OPS-11: `readiness_settle_ms` vs kubelet probe period

Owner: div-ops.
Finding: ROUND8-OPS-11 (medium) — `readiness_settle_ms` default 1000 is below typical kubelet `periodSeconds: 10`; pod transitions to Terminating while still listed Ready in Endpoints; the *next 10 seconds of new connections* land on the draining pod.

Coverage-gap theme cited: **Theme 2 — Operational-vs-laboratory test posture.** Kubelet probe-period interaction only visible against a real K8s control plane. Round 1-7 ran against single-instance setups.

## A. The decision tree

Three options, ordered by ambition:

1. **Raise the default** to a value that works under a default-config kubelet.
2. **Implement a "readiness flip ack"** that waits until at least one full probe interval has elapsed since the first 503 served.
3. **Both**: raise the default AND ship the flip-ack as a future-work option.

We adopt **Option 1 for v1** (the bounded behavioural change) and **stage Option 2 as a follow-up** (the operator-quality improvement). Option 1 alone closes the bug for the default-kubelet case. Option 2 is the right answer in the long run but requires plumbing probe timestamps which is a more invasive change.

## B. Option 1 — raise the default

`crates/lb-config/src/lib.rs`: change `default_readiness_settle_ms() -> u64 { 1000 }` to `default_readiness_settle_ms() -> u64 { 11_000 }`.

Rationale for 11_000: kubelet default `periodSeconds: 10`. 11 s guarantees at least one probe falls inside the window even in the worst case where set_draining fires immediately after a probe (probe scheduled 10 s later + 1 s margin). Pingora's `CLOSE_TIMEOUT=5s` is for signal-handler settling on a single-instance basis; our case is the K8s upstream-LB lameduck which is the dominant deployment mode.

The new default is *additive* to the 10 s drain budget — total deploy time per pod goes from `1 s (settle) + 10 s (drain) = 11 s` to `11 s + 10 s = 21 s`. Still well under typical `terminationGracePeriodSeconds: 30`.

Validation range: keep `100..=60_000` ms (already in lib.rs). Operators with non-standard kubelet tuning can override.

## C. Option 2 — readiness flip ack (follow-up, design only)

Documented here so the follow-up PR has a concrete target.

In `crates/lb-observability/src/probes.rs`:
```rust
pub struct ProbeRegistry {
    state: AtomicU8,
    // Round-8 addition (OPS-11):
    set_draining_at: AtomicI64,        // monotonic-ns timestamp of set_draining(true)
    first_503_seen_at: AtomicI64,      // monotonic-ns timestamp of first 503 served on /readyz
    last_503_seen_at: AtomicI64,       // monotonic-ns timestamp of most recent 503 on /readyz
    observed_probe_period_ms: AtomicU64, // EMA of (this_503 - prev_503) since draining
}
impl ProbeRegistry {
    pub fn flip_acked(&self, min_period_ms: u64) -> bool {
        let drain_at = self.set_draining_at.load(Acquire);
        let first = self.first_503_seen_at.load(Acquire);
        if drain_at == 0 || first == 0 { return false; }
        let now = Instant::now().nanos_since_zero();
        (now - first) >= (min_period_ms as i64 * 1_000_000)
    }
}
```

In `crates/lb-observability/src/admin_http.rs` `/readyz` handler: on every 503 response (i.e. `state == Draining`), update `last_503_seen_at` and (if 0) `first_503_seen_at`. Compute `observed_probe_period_ms` as EMA of inter-503 timestamps.

In the drain coordinator (per OPS-04+L4-12 phase 3 ReadinessSettle): wait until `min(spec.readiness_settle, probes.flip_acked_or_timeout(observed_probe_period_ms))`. Fall back to `spec.readiness_settle` as the hard cap (always).

Emit:
- `readiness_settle_late_total` counter — bumped if the coordinator reaches phase 4 (ListenerCancel) without having served any 503 on `/readyz`. The visible signal "your settle window was too short."
- `readiness_observed_probe_period_seconds` gauge — what we observed during the drain.

Add response header `X-LB-Draining-Since: <duration-ms>` on `/readyz` 503 responses so operators have a manual debugging pivot.

This is Option 2 *design*; v1 ships Option 1 only.

## D. Documentation

### D.1 — `CONFIG.md`

Add to the runtime config doc:
> `readiness_settle_ms` (default 11000) — wall-clock duration the drain coordinator waits between flipping `/readyz` to 503 and starting to cancel listeners. The default is sized for kubelet `periodSeconds: 10` so at least one probe falls inside the window. For aggressively-tuned kubelets (`periodSeconds: 1`), 1500 ms suffices; for higher-period external LBs (some cloud LBs poll every 30 s), set to ~31000.

### D.2 — `RUNBOOK.md` "Drain" section

> ### Tuning `readiness_settle_ms`
>
> The lameduck window must be at least 1 full upstream-probe period plus margin. Defaults:
>
> | Upstream LB | Typical probe period | Recommended `readiness_settle_ms` |
> |---|---|---|
> | kubelet (default) | 10 s | 11000 (default) |
> | kubelet, aggressive | 1 s | 1500 |
> | AWS ALB / NLB | 30 s | 31000 |
> | GCP Load Balancer | 5 s | 6000 |
>
> If you see traffic landing on a draining pod (`accept_inflight` rising after `set_draining=true`), the window is too short.

### D.3 — `RUNBOOK.md` `LbReadinessSettleLate` alert (Option 2, future)

> Stub: when `readiness_settle_late_total` is exposed, the alert
> `rate(readiness_settle_late_total[5m]) > 0` indicates the configured
> settle window is shorter than the actual upstream-probe period.

## E. Tests

- `tests/probes_settle.rs::default_settle_within_kubelet_period` — start lb with default config; trigger drain; assert a 503 is served on `/readyz` before phase 4 begins (simulated kubelet 10 s probe).
- `tests/probes_settle.rs::custom_settle_aggressive_kubelet` — override `readiness_settle_ms=1500`; assert phase 3 completes in <1.5 s.
- `tests/runbook_defaults.rs::test_readiness_settle_default_value` — RUNBOOK kubelet-default row matches `default_readiness_settle_ms()`.

## F. Verification

- `cargo test test_readiness_settle_default_value` passes; RUNBOOK and source agree.
- The OPS-04+L4-12 drain coordinator phase 3 uses the per-config value.
- Operator running on default-config K8s sees no new-traffic-on-draining-pod (manual smoke; not CI-runnable).

## G. REL-2-04 status update

REL-2-04 (probe split + `/livez` / `/readyz` separation) is `Verified-Fixed` per the prior register. The OPS-11 finding adds a *parameterisation* gap, not a regression. Status note update:
> Verified-Fixed; the Round-8 OPS-11 plan raises `readiness_settle_ms` default from 1000 to 11000 to accommodate default kubelet probe periods. Future work (Option 2): flip-ack model with `readiness_settle_late_total` counter; tracked separately.

## H. Risk

- Operators running on cgroups-v1 / non-systemd-managed restarts where `terminationGracePeriodSeconds` is <11 s see the new default exceed the budget. The OPS-04+L4-12 drain coordinator's phase 6 (XdpDetach) still runs because phase 3 is a sleep that respects the parent token; if SIGKILL arrives, the process dies with stranded XDP (the OPS-01+L4-12+L4-04 stale-self recovery handles this on next start).
- We do not raise to 30 s because that aligns *exactly* with the typical `terminationGracePeriodSeconds: 30` — there must be drain budget left over. 11 + 10 = 21 leaves 9 s of headroom.
