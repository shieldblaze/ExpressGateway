# Plan for ROUND8-L7-08 — Send explicit RST_STREAM(CANCEL) and mark connection unreusable on H2 upstream read timeout

Finding-ref:   ROUND8-L7-08 (high, status: Open)
Files touched:
  - `crates/lb-io/src/http2_pool.rs`        (`send_request` line 199–210, `evict`, `peers`)
  - `crates/lb-l7/src/h2_to_h2.rs`          (caller of `send_request`)
  - `crates/lb-l7/src/h1_to_h2.rs`          (caller of `send_request`)
  - `crates/lb-observability/src/prometheus_exposition.rs`  (new counter)
  - new test file: `crates/lb-l7/tests/round8_h2_upstream_cancel.rs`

Approach (≤500 words):

Reference: **Pingora 0.8.0 CHANGELOG** — "Send RST_STREAM CANCEL on
application read timeouts for h2 client" + handoff Top-10 defensive
pattern #5. The reasoning: a `NO_ERROR` or implicit reset on drop
looks indistinguishable to the upstream from a misbehaving peer, and
upstream `max_pending_accept_reset_streams` defences (post-CVE-2023-44487)
will count *us* as the attacker. An explicit `CANCEL` carries the
"this was an application-level read timeout, not a protocol error"
signal.

hyper 1.x `SendRequest::send_request` does not expose `send_reset`
directly. The practical pattern is to **abort the inflight stream
future via a `CancellationToken`** rather than rely on `tokio::time::
timeout`'s implicit-drop semantics. hyper's H2 layer, when the stream
future is aborted via cancellation, emits a `Reset(CANCEL)` frame
because the abort happens *before* hyper's drop completes; with
`tokio::time::timeout` returning `Err(Elapsed)`, the drop ordering is
the next scheduling tick and the reset is `NO_ERROR`.

Concrete fix:

1. In `http2_pool::send_request`, replace the `tokio::time::timeout`
   call with a manual `select!`:
   ```rust
   let cancel = CancellationToken::new();
   let sender = ...;
   let send_fut = sender.send_request(req);
   tokio::select! {
       res = send_fut => res,
       _ = tokio::time::sleep(timeout_dur) => {
           cancel.cancel();
           // Trigger immediate stream cancel by dropping the
           // SendRequest future *and* the response future
           // before returning. hyper observes the drop within
           // the same task wake.
           self.evict_with_reset(addr);
           return Err(Http2PoolError::Timeout);
       }
   }
   ```
2. Add `evict_with_reset` to `Http2Pool` that:
   - Removes the sender from `peers`.
   - Increments `lb_h2_upstream_cancel_total{reason="read_timeout"}`.
   - Drops the `SendRequest` handle — hyper's connection-driver task
     emits `RST_STREAM(CANCEL)` for any inflight stream associated
     with this handle.

3. After the cancel, mark the entire connection unreusable (Pingora
   does both — "the timeout suggests the upstream hung on this
   stream specifically; the connection might be sick"). Already
   handled by removal from `peers`; ensure no other in-flight
   stream-borrows have stale references.

4. **Verify hyper's drop semantics** by reading `hyper/src/client/conn/http2/mod.rs` for `Builder::Send::poll` — confirm
   the assumption that drop-before-completion emits CANCEL. If
   hyper does not give us this guarantee on the current version, the
   fallback is to track this as a deferred item until hyper exposes
   an explicit `send_reset` method (there is an active issue on
   `hyperium/hyper` discussing this). Document the fallback in
   `audit/deferred.md`.

Reference pattern: Pingora `pingora-core/src/protocols/http/v2/client.rs` `H2ConnectionRef::drop` uses an explicit `reset` call before drop. Lifting the *pattern* (not the code) is the intent.

**Boundary disclosure:** this plan is entirely inside `lb-io` and the
two L7 caller crates. No `lb/src/main.rs` changes.

Proof:
  - `round8_h2_upstream_cancel::timeout_emits_cancel_reset` — invariant: a backend that holds a stream open past the configured `send_timeout` receives an `RST_STREAM` with error code `CANCEL` (0x8). Verified by a faux H2 backend that snapshots received frames.
  - `round8_h2_upstream_cancel::other_streams_not_collateral_damage` — invariant: with two concurrent streams on the same connection where one times out, the other completes normally (i.e. cancel is scoped to the timed-out stream, not the whole connection — *except* we also evict the connection from the pool so a *new* stream-acquire goes to a fresh connection).
  - `round8_h2_upstream_cancel::evicted_connection_not_reused` — invariant: after a timeout-induced eviction, the next `acquire(addr)` returns a *different* `H2Conn` handle (verified by pointer identity or a fresh-connection counter).
  - `round8_h2_upstream_cancel::cancel_counter_increments` — invariant: the new Prometheus counter increments exactly once per timeout.

Risk / blast radius:
  - hyper 1.x version-specific drop-emits-CANCEL behaviour is the
    main risk. If hyper changes drop semantics, the test
    `timeout_emits_cancel_reset` will catch the regression.
  - Connection eviction on every read timeout is correct per the
    reference but reduces pool utilisation if upstream timeouts are
    transient. Operators with chatty-but-healthy upstreams may want
    a per-eviction backoff; out of scope for this plan, leave as a
    deferred knob.

Cross-ref:
  - L7-07 + L7-12 (H2 glitches counter) — the CANCEL we emit also
    contributes to the *upstream's* glitch counter (if it also runs
    HAProxy / Envoy logic). Not our problem to solve, but the cross-ref
    documents the symmetry.
  - L7-10 (body over-read marks connection non-reusable) — same
    "evict the connection on stream-level error" pattern.

**Audit-failure-mode theme:**
  - **Theme 4 — Multi-validator audit handoff**: CODE-2-09 ported
    `acquire_async` and validated the pool acquisition path. The
    *error-path* (timeout, reset) was never re-walked by the
    protocol validator. Pingora paid for this exact bug in 0.8.0
    and the lesson did not transfer.
  - **Theme 1 — "Verified-Fixed" snapshot of script existence, not
    capability**: the pool's `evict` API existed at `Verified-Fixed`
    time; whether the evict *also-resets* was not in the audit
    surface.

Owner:           div-l7
Lead-approval: approved 2026-05-14 team-lead-r8
