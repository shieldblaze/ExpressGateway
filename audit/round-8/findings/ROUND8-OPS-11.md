### ROUND8-OPS-11 — `/readyz` flip-to-503 has no inflight grace inside the *current* probe scrape window; readiness_settle_ms is the only buffer

Reference: `audit/round-8/research/envoy.md` lesson 19 ("graceful close timing must be jittered"); generic Kubernetes lameduck pattern (kubelet probe period typically 10 s; `terminationGracePeriodSeconds` typically 30 s). The handoff `cross-cutting items` 1 ("Hot reload semantics — listener handover model") cites Cloudflare's Oxy article on the same.
Our equivalent: `crates/lb/src/main.rs:1926-1934` — drain sequence flips `probes.set_draining()` then sleeps `readiness_settle_ms` (default 1000 ms). Probe registry is a single `AtomicU8` (`crates/lb-observability/src/probes.rs:71`). No "readiness flip ack" — we sleep a fixed wall-clock duration regardless of whether the upstream LB has scraped `/readyz` even once during that window.

Severity: medium
Status:   Verified-Fixed(verifier=verify, d13e7d28)   <!-- default_readiness_settle_ms() returns 11_000 (lib.rs:480-482), raised from 1000; = kubelet default periodSeconds:10 + 1s margin so >=1 /readyz 503 lands in window. Exceeds kubelet probe period as required. RUNBOOK tuning table added. See audit/round-8/verify/ops.md. -->
Status (pre-verify):   Open

Divergence:
- Envoy and HAProxy expect the LB-above (cloud LB, ingress controller, etc.) to scrape `/readyz` on a schedule typically configured 5..10 s. The lameduck idiom: flip ready=false → wait until "long enough that at least one probe occurred" → drain.
- 1000 ms is materially shorter than the typical kubelet probe period (10 s default `periodSeconds`). An aggressively-tuned kubelet (`periodSeconds: 1`) catches it; a default-config kubelet does not — the pod transitions from `Ready` to `Terminating` while still listed as `Ready` in the Endpoints object, and traffic continues to be routed there for up to 10 s after `set_draining()`.
- The drain budget (10 s, ROUND8-OPS-10) is now half-consumed by lameduck signalling that did not work.

Impact:
- For typical Kubernetes deployments (kubelet probe period = 10 s, no aggressive tuning), the readiness flip is effectively invisible. The pod drains in-flight connections gracefully, but the *next 10 seconds of new connections* land on the draining pod and are also drained-mid-request.
- Operator perception: a deploy "works" in terms of the rolling-restart progress bar but emits a brief 5xx blip from every replica it rotates through.

Recommendation:
1. Default `readiness_settle_ms` from 1000 to **`probe_period_ms` × 2 + jitter** where `probe_period_ms` is observed from `/readyz` scrape timings.
2. Better: implement a "readiness flip ack" — record `Instant` of every `/readyz` 503 served. The drain sequence waits until *one full probe interval has elapsed* between the first 503 and now (heuristic: `(now - first_503_seen) >= probe_period_ms`). Fall back to `readiness_settle_ms` as a max.
3. Document the formula in `RUNBOOK.md` Drain section: "readiness_settle_ms should be ≥ 2 × kubelet probe period; default 1000 ms assumes operator has tuned periodSeconds: 0.5".
4. Add `readiness_settle_late_total` counter — bumped if the drain sequence reaches the cancel step without having served any 503 on `/readyz`. This is the visible signal "your settle window was too short".
5. Expose `/readyz` 503-since-set_draining duration as a header on the response (`X-LB-Draining-Since`) so operators have a debugging pivot for "did kubelet actually see the 503?".

Notes:
- Pingora explicitly documents `CLOSE_TIMEOUT=5s` as the *minimum* between flip and close to let signal handlers settle (separate from the EXIT_TIMEOUT for in-flight traffic). 1 s is below that minimum.
- This was *not* caught by Round 1..7 because the audit ran against a single-instance test setup; kubelet probe-period interaction is only visible against a real K8s control plane.
