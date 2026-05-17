### ROUND8-OPS-04 — TCP listener accept loop has no cancel arm; relies on `JoinHandle::abort()` mid-accept

Reference: `audit/round-8/research/pingora.md` lesson 18 ("Server::run_forever now takes ownership and enforces exit semantics … making the entry function consume `self` prevents 'I already drained, why am I dropping again' footguns"); `audit/code/round-2-review.md` CODE-2-03 was supposed to fix exactly this and is marked Verified-Fixed.
Our equivalent: `crates/lb/src/main.rs:2179-2211` — the per-listener `loop { match listener.accept().await { ... } }` has NO `tokio::select!` arm on `state.shutdown_token` (or a per-listener cancel token). The drain sequence at `crates/lb/src/main.rs:1942-1944` calls `for h in &listener_handles { h.abort(); }` — i.e. exactly the `JoinHandle::abort()` pattern CODE-2-03 was supposed to retire.

Severity: medium
Status:   Verified-Fixed(verifier=verify, 698c5a63)   <!-- round8_drain_15case 16/16, round8_drain_coordinator 6/6, legacy shutdown 3/3 + per_connection_drain 4/4 green. main.rs accept loop has biased select on listener_cancel_token + synchronous post-accept is_cancelled() shutdown BEFORE the per-IP gate (closes C-3 fd/counter drift). 15 cases distinctly asserted (not hollow). See audit/round-8/verify/ops.md. -->
Status (pre-verify):   Open

Divergence:
- CODE-2-03 (`audit/code/round-2-review.md:128-180`) prescribed: "The accept loop becomes `tokio::select! { _ = cancel.cancelled() => break, acc = listener.accept() => acc }`". This pattern was applied to: admin HTTP listener (`crates/lb-observability/src/admin_http.rs:246-253`), QUIC listener tasks, sampler tasks. It was NOT applied to the TCP/H1/H1s accept loop.
- The Round-7 audit register marked CODE-2-03 `Verified-Fixed(9ff2b9b, fc050b0, bca4285)`. The status note specifically claims "all 5 previously-bypassing `tokio::spawn` sites now route through `shutdown.tracker().spawn(...)` — listener accept loop, …". Spawning through the tracker is correct, but the listener accept *loop itself* still aborts via `JoinHandle::abort()`.
- Per-connection tasks are properly cancellable (`crates/lb/src/main.rs:2306` clones `shutdown_token`, line 2484 has a `biased select!` against `conn_cancel.cancelled()`). The listener task that owns the `accept().await` future does not.

Impact:
- A SIGTERM during a pending `accept().await` future calls `JoinHandle::abort()` on that task. `tokio::task::JoinHandle::abort` causes the future to wake and return as cancelled at the next poll. For an `accept().await` future this happens immediately (the future has nothing to clean up). The behaviour is correct in this happy case.
- The non-happy case is when an accept *just* succeeded but the post-accept work (admission gate, semaphore, spawn) is mid-flight. `JoinHandle::abort` cancels mid-step — the accepted socket might be open with no per-conn task spawned, the IP-cap counter might be incremented without a matching decrement, the inflight semaphore is unaffected because the permit was never acquired. The net is leaked accepted sockets and per-IP counter drift on every drain.
- Compounded with ROUND8-OPS-02: every drain creates this race on every listener; we have not seen the bug because we drain rarely.

Recommendation:
1. Replace `listener.accept().await` at `crates/lb/src/main.rs:2180` with:
   ```rust
   tokio::select! {
       biased;
       () = state.shutdown_token.cancelled() => {
           tracing::info!(address = %bind_addr, "listener cancelled by drain");
           return Ok(());
       }
       res = listener.accept() => res,
   }
   ```
2. Remove the `JoinHandle::abort()` loop at line 1942-1944; instead `await`-join each handle with a bounded timeout, falling back to abort only on overflow (this is the symmetric construction to `Shutdown::drain` for per-connection tasks).
3. Update CODE-2-03 status comment to reflect partial fix: the per-connection tracker plumbing landed, but the accept-loop cancel arm did not. Reverify against the recommendation.
4. Add `tests/reload_zero_drop.rs::test_listener_cancel_clean` — assert that after `set_draining` + cancel, the accept task returns within `readiness_settle_ms + 100ms` rather than relying on abort.

Notes:
- Why this slipped: Round 4 audited the per-connection lifecycle (which is the *consequential* path, traffic-wise) and validated that the per-conn task tracker fires. Nobody walked back up to the accept loop because the accept future was assumed to be cancellable-at-poll. It is, but the wrapper is not cooperative — and the per-listener `JoinSet` / cancel-token model was the entire point of CODE-2-03.
