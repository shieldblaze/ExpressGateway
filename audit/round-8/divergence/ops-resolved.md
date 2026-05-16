# Round 8 Operability — different-but-fine

Areas walked against the references that are deliberate divergence with sound rationale.

---

## Cert rotation locks handshake to bundle (REL-2-03 implementation, against Envoy SDS pattern)

Reference: `audit/round-8/research/envoy.md` lesson 18 (hot restart with parent/child overlap, stats merged) — Envoy's SDS rotates by FD-passing the new SecretConfigSource into a fresh listener. Pingora's `pingora-rustls` uses `ResolvesServerCert` with per-handshake resolution.

Our equivalent: `crates/lb/src/main.rs:2337-2344` and `:2401-2412` — the per-listener `ArcSwap<TlsConfigBundle>` is snapshotted *once per accept* via `bundle.load_full()` and the resulting `Arc<ServerConfig>` is then cloned into `TlsAcceptor::from(...)`. The acceptor (and hence the cert chain + key for this connection) is locked-in for the lifetime of the handshake.

Why this is fine:
- A SIGUSR1 mid-handshake atomically replaces `bundle` to point at a new `Arc<TlsConfigBundle>`. The in-flight handshake holds an `Arc::clone` of the *previous* `server_config` (cloned at line 2344). The old `ServerConfig` Arc stays live until the handshake completes. There is no "cert A handshake completes with cert B" race.
- The new bundle is picked up at the *next* accept. New connections see new cert; in-flight handshakes see old cert. This is the exact invariant Envoy SDS provides via a different mechanism.
- Tests in `crates/lb-security/src/cert_rotation.rs` (per the REL-2-03 fix commit `334b69a` referenced in the audit register) explicitly assert this snapshot-stability against concurrent reload.

Divergence from references is intentional: we picked the simpler `ArcSwap`-per-accept model over Envoy's overlap-listener model because we do not yet implement FD-passing reload (see ROUND8-OPS-01). Once FD-passing lands, the cert rotation story stays the same — old process keeps serving with cert A on its inherited FDs, new process binds and serves cert B.

Confidence: high. The Arc lifecycle is a Rust correctness property; the test suite enforces it.

---

## Probe registry is a single AtomicU8, not a per-listener state machine

Reference: `audit/round-8/research/envoy.md` ("listener manager" — Envoy's listener-readiness is per-listener); generic K8s health-probe model.

Our equivalent: `crates/lb-observability/src/probes.rs:71` — a single `AtomicU8` encodes the *process-wide* lifecycle state. `/readyz` returns one bit (`Ready` or `Draining`); the registry has no per-listener disposition.

Why this is fine:
- Our drain semantics are all-or-nothing — we don't support partial drain (drain listener A while keeping listener B). If we did, the per-listener state would be needed.
- K8s readiness probes are *pod-scoped*, not listener-scoped — a single 503 from `/readyz` removes the entire pod from the Service Endpoints. The granularity of the probe matches the granularity of the K8s primitive.
- The acquire/release ordering on `set_ready`/`set_draining` (lines 105-121) is correct: `Ordering::AcqRel` on the CAS prevents a draining process from CAS-ing back to ready.

If we ever add per-listener drain (deploy a single listener config change without restarting), this becomes a finding. For now: deliberate simplicity.

---

## /livez always returns 200 (never flips to 503) — matches K8s liveness convention

Reference: `audit/round-8/research/envoy.md` (no equivalent — Envoy doesn't ship K8s-shaped probes). The K8s docs themselves: liveness probes should NOT include downstream dependency checks; they are "is the process up".

Our equivalent: `crates/lb-observability/src/probes.rs:131-137` — `is_live()` returns `true` always. `/livez` returns 200 even during drain.

Why this is fine:
- K8s liveness 503 → SIGKILL. During drain, a 503 on `/livez` would have kubelet kill the pod *during graceful drain*, defeating the entire drain sequence. That's the bug pattern the REL-2-04 fix consciously avoided.
- The split is correct: `/livez` = "should kubelet kill me" (never during drain); `/readyz` = "should the LB above route to me" (false during drain).
- `is_live` returning `true` unconditionally is *correct* — the only way `is_live` would be false is if the process were dead, in which case the endpoint also wouldn't answer.

---

## Single-threaded probe state (no per-listener bind tracking against REL-2-04 spec line 224)

Reference: REL-2-04 recommendation 2 in `audit/reliability/round-2-review.md:226-230` — "200 only if every configured listener is currently bound and accepting (track per-listener `Arc<AtomicBool>` `accepting` flag, set true after first successful `accept()`, set false at the start of drain)."

Our equivalent: `crates/lb-observability/src/probes.rs` — no per-listener `accepting` bool. `/readyz` returns 200 if state == `Ready` (which is set after `bind` of all listeners completes, not after the first `accept`).

Why this is fine (with caveat):
- "Bound" and "accepting" are nearly equivalent for tokio's `TcpListener` — once `bind` returns, the kernel queues incoming connections; `accept` is the next syscall to dequeue. The window between bind-success and first-accept is one event-loop tick.
- The REL-2-04 spec recommendation 2 was the more conservative form. Our simpler form trades a sub-millisecond accuracy window for code simplicity.
- The bug REL-2-04 spec was guarding against was "one listener failed to bind, the process still returns 200". Our code returns 200 only if *every* listener bind succeeded (otherwise the binary exits with an error before flipping `set_ready`). Same end result.

Caveat: this is fine *for now*. If we ever add per-listener health checks (a listener becomes unhealthy mid-life because its socket got into a broken state — extremely rare), the per-listener bool would be needed.

---

## Panic = unwind + set_hook (not panic = abort), against CODE-2-02

Reference: `audit/code/round-2-review.md` CODE-2-02 status `Verified-Fixed(120e4fa, b6aeea5)` — "panic=abort in release+bench; dev/test deliberately keep unwind for proptest/loom."

Our equivalent: `Cargo.toml` `[profile.release]` sets `panic = "abort"`. `crates/lb/src/main.rs:100-150` installs the panic hook.

Why this is fine:
- The combination chosen (abort + hook) is the same shape Cloudflare's runtime ships. The hook logs first (`tracing::error!` with location + backtrace + bump `panic_total`), then the runtime aborts because `panic = "abort"`.
- The audit-register split between unwind (dev/test) and abort (release) is deliberate — proptest and loom both rely on `catch_unwind` to recover from shrunk test cases, which `panic = "abort"` would defeat. Test profile correctly keeps unwind.

---

## ProbeRegistry exposes `is_started` derived from state, not a separate startup flag

Reference: Generic K8s startup-probe convention — `startupProbe` should distinguish "still booting" from "ready". Three states (Starting, Ready, Draining) on one AtomicU8 collapse Starting and Draining into "not ready" from the `/startupz` perspective.

Our equivalent: `crates/lb-observability/src/probes.rs:142-145` — `is_started` = `!matches!(state(), Starting)`. So during `Draining`, `/startupz` returns 200 (i.e. "we did finish startup at some point").

Why this is fine:
- `/startupz` is K8s's "did you finish booting yet" signal — once a 200 is observed, kubelet stops calling `/startupz` and switches to `/livez` and `/readyz`. The semantics of "200 once startup completed at least once" matches the K8s contract.
- A draining pod has, by definition, already booted. Returning 200 on `/startupz` is correct.

---

## TaskTracker-based drain instead of JoinSet (CODE-2-03 plumbing)

Reference: `audit/code/round-2-review.md` CODE-2-03 recommendation — "moved behind a `tokio_util::task::TaskTracker` (preferred over hand-rolled `JoinSet` because it lets you `wait()` for the trailing fan-out)."

Our equivalent: `crates/lb-core::Shutdown` uses `TaskTracker` and exposes `drain(deadline) -> DrainOutcome`. The reference recommended TaskTracker explicitly, and we picked it.

Why this is fine: matches the recommendation exactly. Note for future: the listener accept loop itself is NOT funneled through the tracker (ROUND8-OPS-04); that's a real finding. The per-connection tasks ARE.

---

## XDP loader BTF-load failure deferred to background (vs eager fatal)

Reference: `audit/round-8/research/_l4_handoff.md` cross-cutting item 3 ("BTF load failure in containers must be non-fatal").

Our equivalent: `crates/lb-l4-xdp/src/loader.rs` uses `EbpfLoader` which is BTF-non-fatal; the binary continues without XDP if BTF fails. Verified by reading the comment at `crates/lb-l4-xdp/src/loader.rs` and the existing `xdp_btf_optional.rs` integration test (per round-7 audit `audit/round-7/SUMMARY.md` gate-matrix output).

Why this is fine: matches the L4 handoff recommendation exactly. Containerised deploys without `/sys/kernel/btf/vmlinux` get a single WARN, not a flood.

---

## Closing thoughts

Most of the major operability primitives walk correctly against the references. The 12 findings filed cover the divergences worth fixing or documenting; this resolved-as-fine list covers the divergences that are deliberate-and-correct.
