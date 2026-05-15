### ROUND8-OPS-02 — Drain has no jitter / randomisation; deploy-wide thundering herd risk

Reference: `audit/round-8/research/envoy.md` lesson 19 (`drain_manager_impl.cc`: `P(close) = elapsed / drain_timeout`, "distributing close events over the first quarter of the drain window … prevents a thundering herd of disconnects from the same LB to the same backend") + defensive pattern 5.
Our equivalent: `crates/lb/src/main.rs:1925-1984` (drain sequence). `grep "jitter\|random\|rand::\|distribute"` in the drain block returns zero hits. `crates/lb-core` `Shutdown::drain` (referenced at `crates/lb/src/main.rs:1966`) waits on `TaskTracker::wait()` with a single deadline — every per-connection task gets its cancel signal at the *same instant* (`shutdown.token().cancel()`).

Severity: medium
Status:   Verified-Fixed-Partial(verifier=verify, c26056b5+147e1121)   <!-- Per-process drain jitter in run_drain (shutdown.rs:325, jitter_millis:711) desyncs replicas vs upstream LB; literal rec asked per-connection jitter — per-conn arm is a disclosed div-l7 follow-up. Non-blocking. See audit/round-8/verify/ops.md. -->
Status (pre-verify):   Open

Divergence:
- Envoy spreads connection-close events probabilistically over the first ¼ of the drain window. The deliberate goal is to avoid all clients of a single Envoy reconnecting simultaneously to whatever the next LB hop is.
- We fire `shutdown.token().cancel()` synchronously at line 1940 and every cooperative `select!` in every per-connection task observes the cancellation in the same scheduler tick. Per-protocol drain (`graceful_shutdown` for H1/H2, `CONNECTION_CLOSE` for H3) does emit polite drain signals — but at exactly the same moment, on every connection.
- When the LB is deployed behind another LB (typical: cloud LB → ExpressGateway → backend), every connection from the cloud LB to one ExpressGateway pod gets a `GOAWAY` / `Connection: close` simultaneously. The cloud LB then retries every connection on alternate pods simultaneously. The replacement pods see a synchronous spike of reconnects.

Impact:
- A rolling restart of N replicas creates N synchronised reconnect spikes against a degraded fleet. Under load, the spikes can collide with one another (replica 1 just finished restart and is healthy, replica 2 starts restart, all of replica 2's traffic lands on replica 1 alongside replica 1's existing load).
- The bug is invisible at small scale (one replica, light traffic) and only bites when the fleet reaches >2-3 replicas + a stateful upstream LB. References hit this in production; we have not deployed at scale yet, so this is a lesson-not-yet-paid-for.

Recommendation:
1. In the per-connection drain `select!` (`crates/lb/src/main.rs:2484-2501`), when the cancel arm fires, sleep for a per-connection random jitter `Duration::from_millis(rand::thread_rng().gen_range(0..(drain_timeout_ms/4)))` before propagating the cancel into the protocol layer.
2. Alternatively, the Envoy `P(close) = elapsed / drain_timeout` model: on each protocol tick after `set_draining`, roll a per-connection RNG; close if `rand < (elapsed / drain_timeout)`. This naturally distributes closes over the entire drain window.
3. Add a metric `shutdown_drain_seconds` histogram (REL-2-02 spec'd it; never landed — see ROUND8-OPS-03) so the operator can confirm the distribution is in fact spread.
4. Document under `RUNBOOK.md` "Drain (graceful shutdown)" section that drain close is jittered, with the upper bound (drain_timeout / 4) called out.

Notes:
- This finding is a "lesson-not-yet-paid-for" in the round-8 stance taxonomy. The bug is in our code waiting to bite the first deploy that exceeds three replicas serving keep-alive H2/H3 traffic at >1 krps. None of our integration tests exercise the multi-replica thundering-herd shape; adding one (spin up 3 instances + a client fleet against all three, restart one, measure RPS divergence on the other two) is the way to keep this fixed once fixed.
