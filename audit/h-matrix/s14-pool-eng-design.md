# S14 pool-eng design — lb-io idle/no-progress send mechanism (Phase 1)

> **Status: DESIGN ONLY — awaiting lead plan-approval before any `.rs` edit.**
> Authoring agent: pool-eng (`feature/cfbw-s14-pool`).
> Base: `01432589` (S13-promoted main tip; H-matrix 9/9).
> Scope: Phase 1 of `s14-cf-body-wallclock-plan.md` §1 §3 §4 — the
> single-sourced `idle_bounded_send` helper + a new
> `Http2Pool::send_request_idle` method, both inside **lb-io**. Cell wiring,
> `HttpTimeouts::head`, and the R12 cross-cell equivalence proof are out of
> scope here (Phase 2 + Phase 3).

---

## 0. Goals and non-goals

### Goals
1. A **generic** `idle_bounded_send` helper that drives an arbitrary pinned send
   future under a **two-phase** deadline:
   - **Phase A (upload-in-flight):** a no-forward-progress idle watchdog,
     reset whenever the request-side pump records a `tx.send` success.
   - **Phase B (upload-complete):** a fixed `head_timeout` cap on the
     remaining wait for the future's `Output`.
2. A **new** `Http2Pool::send_request_idle(addr, req, last_progress,
   upload_complete, idle, head_timeout)` method built on the helper. The
   EXISTING `Http2Pool::send_request` stays bit-identical (backwards-compat
   for `h3_bridge.rs:2573` and existing tests).
3. Pure deterministic isolation tests using `tokio::time::pause` /
   `start_paused`; the helper testable WITHOUT the pool.
4. Workspace-wide compile clean; scoped llvm-cov ≥80% per-line on the new
   `idle_send.rs` module and the new pool method ranges.

### Non-goals (Phase 2 / Phase 3 owners)
- Wiring the 4 cells (builder-1 in Phase 2).
- Adding `HttpTimeouts::head` to the config struct (builder-1).
- R12 cross-cell equivalence proof (verifier in Phase 3).
- Touching `Http2Pool::send_request` callers — they are NOT migrated in Phase 1.
- Per-cell load-bearing 504-pre-fix tests (Phase 3).

---

## 1. Module location and surface

### 1.1 New file

`crates/lb-io/src/idle_send.rs` — re-exported from `crates/lb-io/src/lib.rs`:

```rust
pub mod idle_send;
```

(no glob re-export; callers `use lb_io::idle_send::{idle_bounded_send, IdleSendError};`)

Rationale: lb-l7 already depends on lb-io (per the plan §3); a free module in
lb-io is reachable from both the new pool method (Class B) and from lb-l7
direct H1-client callers (Class A, Phase 2). Keeping the helper as a free
function (not a pool method) preserves the plan's invariant "ONE watchdog
implementation, reused by both classes".

### 1.2 Public surface added in Phase 1

```rust
// crates/lb-io/src/idle_send.rs

/// Outcome of an [`idle_bounded_send`] call.
///
/// The success arm carries the wrapped future's own `Output`; the error arm
/// distinguishes the TWO timeout phases so cell-level callers can attribute
/// a 504 to upload-stall vs. head-stall in logs/metrics.
#[derive(Debug, thiserror::Error)]
pub enum IdleSendError {
    /// Upload phase: no `tx.send` success was observed for `idle` while the
    /// wrapped future remained pending → wedged upload.
    #[error("upload idle timeout: no forward progress for {0:?}")]
    IdleTimeout(std::time::Duration),
    /// Head phase: the pump signalled upload-complete, but the wrapped future
    /// failed to resolve within `head_timeout` afterwards → slow backend head.
    #[error("head timeout: upload complete, no head for {0:?}")]
    HeadTimeout(std::time::Duration),
}

/// Run a pinned send future under a two-phase idle/head deadline.
///
/// `last_progress` is a monotonic millis-since-epoch counter that the
/// REQUEST-EGRESS pump bumps on every successful chunk hand-off into the
/// bounded body channel — i.e. the same event the R8 in-flight gauge already
/// records. `upload_complete` is set ONCE by the same pump at the moment it
/// hands off the terminal frame (clean EOF / End{trailers}); after that flip
/// the helper switches from the idle watchdog to a fixed `head_timeout` cap.
///
/// The helper holds NO body bytes (it only READS the atomic counter and the
/// atomic bool), so memory bound and channel backpressure are unchanged
/// (Phase 1 R8 argument, plan §4).
///
/// `epoch` is the `tokio::time::Instant` captured at request start; all
/// `last_progress` values are millis-since-`epoch`. Using `tokio::time`
/// (not `SystemTime`) makes the helper test-clock-friendly under
/// `tokio::time::pause` / `start_paused`.
///
/// # Errors
/// - [`IdleSendError::IdleTimeout`] — Phase A wedge.
/// - [`IdleSendError::HeadTimeout`] — Phase B wedge.
pub async fn idle_bounded_send<F, T>(
    send_fut: F,
    last_progress: std::sync::Arc<std::sync::atomic::AtomicU64>,
    upload_complete: std::sync::Arc<std::sync::atomic::AtomicBool>,
    epoch: tokio::time::Instant,
    idle: std::time::Duration,
    head_timeout: std::time::Duration,
) -> Result<T, IdleSendError>
where
    F: std::future::Future<Output = T>;
```

### 1.3 New pool method on `Http2Pool` (in `http2_pool.rs`)

```rust
impl Http2Pool {
    /// Idle-aware H2 send.
    ///
    /// Functionally equivalent to [`Self::send_request`] except the fixed
    /// `send_timeout` wall-clock is replaced by the two-phase idle/head
    /// deadline of [`crate::idle_send::idle_bounded_send`]. The same
    /// ROUND8-L7-10 eviction policy applies on both error arms (Send-class
    /// error AND either timeout) to preserve the H2-multiplex corruption
    /// guard documented on [`Self::send_request`].
    ///
    /// `last_progress` / `upload_complete` are owned and driven by the
    /// caller's body pump (see plan §1.0); the pool only consumes them.
    /// `epoch` is the request-start instant the caller uses to derive
    /// `last_progress` millis.
    ///
    /// # Errors
    /// - [`Http2PoolError::Dial`] / [`Http2PoolError::Handshake`] — as in
    ///   `send_request`.
    /// - [`Http2PoolError::Send`] — hyper Send-class error; entry evicted.
    /// - [`Http2PoolError::Timeout`] — either idle OR head deadline fired;
    ///   entry evicted. (Phase 1 keeps a single timeout error variant for
    ///   on-the-wire compat with the existing `Http2PoolError`; the
    ///   `IdleSendError` discriminant is logged at warn-level then collapsed.
    ///   Phase 2/3 may split the variant if the cells want phase-attribution
    ///   on the wire — flag for owner.)
    pub async fn send_request_idle(
        &self,
        addr: std::net::SocketAddr,
        request: hyper::Request<H2ReqBody>,
        last_progress: std::sync::Arc<std::sync::atomic::AtomicU64>,
        upload_complete: std::sync::Arc<std::sync::atomic::AtomicBool>,
        epoch: tokio::time::Instant,
        idle: std::time::Duration,
        head_timeout: std::time::Duration,
    ) -> Result<hyper::Response<hyper::body::Incoming>, Http2PoolError>;
}
```

The existing `send_request` body is **unchanged**; both methods share
`acquire_sender` / `dial_and_handshake` / `evict`. No `Http2PoolConfig` field
is added in Phase 1 — `idle` and `head_timeout` are call-site arguments so
Phase-2 wiring controls them via `HttpTimeouts` (builder-1's surface).

---

## 2. Signaling shape — `last_progress` / `upload_complete`

### 2.1 Encoding

- `last_progress: Arc<AtomicU64>` — millis since `epoch`, where `epoch:
  tokio::time::Instant` is captured by the caller at request start.
- `upload_complete: Arc<AtomicBool>` — false at construction, set to true by
  the pump exactly once when the terminal frame (clean EOF / `End{trailers}`)
  has been pushed into the body channel.

### 2.2 Memory ordering

- Pump writes:
  - `last_progress.store(now_ms, Ordering::Relaxed)` after a successful
    `tx.send`.
  - `upload_complete.store(true, Ordering::Release)` at terminal frame.
- Helper reads:
  - `last_progress.load(Ordering::Relaxed)` in the watchdog tick (no
    synchronization required — staleness ≤1 tick is harmless; the watchdog
    re-arms on the next tick if a bump landed late).
  - `upload_complete.load(Ordering::Acquire)` in the select branch (pairs
    with the pump's Release so the helper sees the final `last_progress`
    bump before observing `upload_complete = true`).

### 2.3 Why this shape (vs. a tokio watch / mpsc / notify)

- **Lock-free, no contention** under high-throughput pumps (the gauge is hit
  per-chunk).
- **No back-channel from helper → pump**: the helper purely observes; if it
  decides to abort, it returns `Err` and the cell-side machinery (Phase 2)
  injects an upstream RST / drops the pump exactly the way today's
  `ProxyErr::Timeout` path does.
- **Phase 2 fit**: lb-l7 pumps already do an atomic bump on the R8 in-flight
  gauge at the same `tx.send` success site — `last_progress` is one extra
  store on the same hot path.
- **Phase 3 fit**: `tokio::time::pause` advances `tokio::time::Instant` but
  NOT `SystemTime` — using the tokio epoch makes the unit tests deterministic
  under a paused clock.

### 2.4 Phase-1 scope reminder

Phase 1 does NOT instantiate `last_progress` / `upload_complete` anywhere
real — the pump-side bumping is Phase 2. Phase 1 tests construct them in the
test harness and drive them directly to verify each helper branch.

---

## 3. The select-loop sketch

Pseudocode (final source will use `tokio::select!` and `tokio::pin!`):

```rust
pub async fn idle_bounded_send<F, T>(
    send_fut: F,
    last_progress: Arc<AtomicU64>,
    upload_complete: Arc<AtomicBool>,
    epoch: tokio::time::Instant,
    idle: Duration,
    head_timeout: Duration,
) -> Result<T, IdleSendError>
where
    F: Future<Output = T>,
{
    tokio::pin!(send_fut);

    loop {
        // Recompute phase + deadline EACH loop iteration so a pump bump
        // landing between iterations is observed on the next tick.
        let complete = upload_complete.load(Ordering::Acquire);

        let deadline = if complete {
            // Phase B: from "first tick after upload-complete was observed"
            // give head_timeout. We anchor on `now()` for the FIRST observed
            // complete, then never re-anchor (so a slow head genuinely fires
            // at head_timeout after upload-done, NOT idle).
            head_deadline_anchor
                .get_or_insert_with(|| tokio::time::Instant::now() + head_timeout)
                .clone()
        } else {
            // Phase A: anchor at last_progress + idle.
            let lp_ms = last_progress.load(Ordering::Relaxed);
            epoch + Duration::from_millis(lp_ms) + idle
        };

        tokio::select! {
            biased; // check send_fut first so a ready future wins over a
                    // simultaneously-firing timer (avoids spurious timeout
                    // races under a paused clock when both are "now").

            out = &mut send_fut => {
                return Ok(out);
            }
            _ = tokio::time::sleep_until(deadline) => {
                if complete {
                    return Err(IdleSendError::HeadTimeout(head_timeout));
                }
                // Re-check: did a bump land AFTER we computed the deadline?
                let lp_ms_now = last_progress.load(Ordering::Relaxed);
                let now = tokio::time::Instant::now();
                let last_progress_instant =
                    epoch + Duration::from_millis(lp_ms_now);
                if now.saturating_duration_since(last_progress_instant) < idle {
                    // Progress happened; re-arm (next loop iteration).
                    continue;
                }
                return Err(IdleSendError::IdleTimeout(idle));
            }
        }
    }
}
```

Notes:
- `head_deadline_anchor: Option<tokio::time::Instant>` is a local in the
  function body (declared above the loop; elided in the sketch for brevity).
- `biased;` is load-bearing under `tokio::time::pause`: when the test
  pre-resolves the send future at the same virtual instant the timer fires,
  the biased ordering ensures success wins (test arm (iv) needs this).
- The `continue` re-arm path is the only loop iteration; under steady
  progress the loop body executes O(N) times where N is the number of
  inter-chunk idle expirations the test forces — in production with a normal
  pump cadence the loop iterates rarely (only when the deadline times out).
- No allocations after the initial `pin!`.

---

## 4. The `Http2Pool::send_request_idle` body

Diff-shape against existing `send_request`: the ONLY structural change is the
`tokio::time::timeout(...)` wrapper replaced by a call to
`idle_bounded_send`. The eviction policy stays bit-identical.

```rust
pub async fn send_request_idle(
    &self,
    addr: SocketAddr,
    request: Request<H2ReqBody>,
    last_progress: Arc<AtomicU64>,
    upload_complete: Arc<AtomicBool>,
    epoch: tokio::time::Instant,
    idle: Duration,
    head_timeout: Duration,
) -> Result<Response<Incoming>, Http2PoolError> {
    let mut sender = self.acquire_sender(addr).await?;
    let send_fut = sender.send_request(request);
    match idle_bounded_send(
        send_fut,
        last_progress,
        upload_complete,
        epoch,
        idle,
        head_timeout,
    )
    .await
    {
        Ok(Ok(resp)) => Ok(resp),
        Ok(Err(e)) => {
            // ROUND8-L7-10 — preserved verbatim.
            self.evict(addr);
            Err(Http2PoolError::Send(e.to_string()))
        }
        Err(idle_err) => {
            // Phase 1 collapses both timeout variants onto the existing
            // Http2PoolError::Timeout. Discriminant is logged for triage.
            tracing::warn!(?idle_err, %addr, "h2 idle/head deadline fired");
            self.evict(addr);
            Err(Http2PoolError::Timeout)
        }
    }
}
```

Note the nested `Ok(Ok(_)) / Ok(Err(_))`: `idle_bounded_send` returns
`Result<T, IdleSendError>` where `T` is the wrapped future's `Output`. For
hyper H2 `send_request` the output is itself a `Result<Response<Incoming>,
hyper::Error>`, hence the nested match — mirroring the existing
`send_request` shape with `timeout(...).await` returning
`Result<Result<...,_>, Elapsed>`.

### 4.1 Zero-regression on existing callers

- `send_request` is untouched. `h3_bridge.rs:2573` and the existing pool
  tests (`http2_pool::tests::pool_starts_empty`,
  `defaults_match_documented_values`) keep their semantics.
- No `Http2PoolConfig` field added → no on-disk config schema change → no
  Phase-2 wiring required just to compile lb-io.
- `Http2PoolError` enum is unchanged → no caller match-arms break.

---

## 5. The `IdleTimeout` error type

Already shown in §1.2: a `thiserror`-derived enum with two variants
(`IdleTimeout(Duration)`, `HeadTimeout(Duration)`).

Rationale for `Duration` payload rather than `()`:
- Test arms can assert the EXACT duration value the helper saw, eliminating
  ambiguity between idle and head firings.
- Future Phase 2 metrics: a timeout-attribution counter labelled by phase
  has the duration to hand.

Rationale for distinct variants (not a single `Timeout(Phase)`):
- `thiserror`'s `#[error("…")]` per-variant string is the cleanest way to
  produce phase-attributed log lines without re-inventing a Display impl.
- The pool's collapsing match (§4) discards the discriminant; cells in
  Phase 2 may keep it.

---

## 6. Unit tests (Phase 1, isolation only — no pool)

All tests use `#[tokio::test(start_paused = true)]` and `tokio::time` virtual
time. The wrapped "send future" is a `tokio::sync::oneshot::Receiver<T>` or a
custom `poll_fn` — both let the test resolve the future at a chosen virtual
instant.

### (i) Chunk-by-chunk progress completes within idle → `Ok(T)`
- `idle = 1 s`, `head_timeout = 5 s`.
- Pump task: every 500 ms (virtual) bump `last_progress` to `now_ms`.
- After 3 s, set `upload_complete = true` and resolve `send_fut(Ok(value))`.
- Assert: helper returns `Ok(value)`, total virtual time ≈ 3 s, no timeout.

### (ii) Wedge from the start → `Err(IdleTimeout)` at idle
- `idle = 1 s`, `head_timeout = 5 s`.
- Pump: NEVER bumps; `last_progress` stays at 0; `upload_complete` stays false.
- `send_fut` never resolves.
- `tokio::time::advance(1100 ms)` then yield.
- Assert: helper returns `Err(IdleTimeout(1 s))`, total virtual time ≈ 1 s.

### (iii) Upload-complete then slow head → `Err(HeadTimeout)` at head_timeout (NOT idle)
- `idle = 500 ms`, `head_timeout = 5 s`.
- Pump bumps once at t=100 ms, sets `upload_complete = true` at t=200 ms.
- `send_fut` never resolves.
- Advance virtual time to t=5500 ms.
- Assert: helper returns `Err(HeadTimeout(5 s))` and the firing instant
  is at ≈ t=200 ms + 5 s = 5200 ms (NOT at t=100 ms + 500 ms = 600 ms,
  which is what a single-phase idle watchdog would do — this is the
  load-bearing two-phase proof).

### (iv) Upload-complete then fast head → `Ok(T)`
- `idle = 500 ms`, `head_timeout = 5 s`.
- Pump bumps at t=100 ms, sets `upload_complete = true` at t=200 ms.
- `send_fut` resolves with `Ok(value)` at t=300 ms.
- Assert: helper returns `Ok(value)` before either deadline.

### (v) Zero-bump (immediate wedge) → `Err(IdleTimeout)` at idle
- `idle = 750 ms`, `head_timeout = 5 s`.
- Pump never bumps; `last_progress` stays at 0; `upload_complete` stays false.
- `send_fut` never resolves; advance to t=800 ms.
- Assert: `Err(IdleTimeout(750 ms))` at t ≈ 750 ms.
  (Distinct from (ii) by the smaller `idle` value, asserting the deadline
  scales with the parameter rather than firing on a hardcoded epoch.)

### (vi) Late bump re-arms the watchdog (regression guard for the `continue` path)
- `idle = 500 ms`, `head_timeout = 5 s`.
- Pump bumps at t=0, then at t=400 ms (re-arms before the t=500 ms fire),
  then NEVER again.
- `send_fut` never resolves.
- Assert: helper fires `Err(IdleTimeout(500 ms))` at t ≈ 900 ms
  (400 + 500), NOT at t=500. Proves the re-arm path actually re-arms.

### (vii) Bump lands AT the deadline tick (race re-check)
- `idle = 500 ms`.
- Pump bumps at t=499 ms; timer fires at t=500 ms; helper re-checks
  `last_progress`, sees an effective `(now - last_progress) = 1 ms < idle`,
  re-arms, and only fires later when no further bump arrives.
- Asserts the "Re-check: did a bump land AFTER we computed the deadline?"
  branch in §3.

### (viii) Pool-method smoke (NO mock backend)
Constructs an `Http2Pool` with a `TcpPool` pointed at an unreachable address
(127.0.0.1:1 closed), calls `send_request_idle` with a tiny `idle = 50 ms`,
and asserts the result is `Err(Http2PoolError::Dial(_))` — i.e. the new
method preserves the existing dial-failure path exactly the way
`send_request` does. This is a wiring smoke test, NOT a behavior test;
behavior is covered by the helper's isolation arms (i)–(vii).

(A "real" pool test that drives an actual h2 backend and tickles the idle
arm belongs in Phase 3 alongside builder-1's cell wiring — Phase 1 keeps
pool-level coverage to a smoke arm to stay focused on the helper.)

---

## 7. Coverage target

- `cargo llvm-cov --workspace --lcov` ([[llvm-cov-workspace-for-depcrate-lines]])
  scoped to:
  - `crates/lb-io/src/idle_send.rs` — all functions.
  - `crates/lb-io/src/http2_pool.rs` — `send_request_idle` line range only
    ([[llvm-cov-session-scope-method]]).
- Per-line ≥80% on both ranges, measured with the session lcov-DA method
  ([[llvm-cov-session-scope-method]]).
- Cross-check vs `cargo llvm-cov --summary-only` for the same module set.

---

## 8. Build/gate plan (Phase 1 only)

1. Author `idle_send.rs` + `pub mod` line + `send_request_idle` method +
   unit tests.
2. `cargo fmt --all`.
3. `cargo clippy --workspace --all-features -- -D warnings`
   ([[cross-crate-gate-scope]] — the helper signature flows through lb-io
   re-exports so a workspace-wide lint sweep is mandatory even though no
   non-lb-io file changes).
4. `cargo test -p lb-io --all-features` for the iteration loop.
5. `cargo test -p lb-io --all-features` ×3 for determinism (the helper is
   timing-sensitive; ×3 catches a flaky virtual-clock arm even though
   `start_paused` is deterministic in theory).
6. `cargo check --workspace --all-features` before declaring done.
7. Scoped llvm-cov per §7.

If (3) or (6) fails the build is rejected before commit; no
"`cargo check -p lb-io` is enough" shortcut ([[cross-crate-gate-scope]]).

---

## 9. Diff-shape summary against current `http2_pool.rs`

| Change | Site | Kind |
|---|---|---|
| `pub mod idle_send;` | `lb-io/src/lib.rs` after `pub mod http2_pool;` | +1 line |
| new file `idle_send.rs` | `lb-io/src/idle_send.rs` | new module |
| `use crate::idle_send::idle_bounded_send;` | top of `http2_pool.rs` | +1 line |
| `Http2Pool::send_request_idle` method | `http2_pool.rs` immediately after `send_request` | +~50 lines, no edits to existing fns |
| unit tests | both new file AND `http2_pool::tests` for the smoke arm | additive |

Existing surface untouched:
- `Http2PoolConfig` (no new field in Phase 1)
- `Http2PoolError` (no new variant in Phase 1)
- `Http2Pool::send_request` (byte-identical)
- `acquire_sender` / `take_alive_sender` / `replace_entry` / `evict` /
  `dial_and_handshake` / `reset_peer` (untouched, shared by both methods)

Zero-regression on `h3_bridge.rs:2573` and existing tests is therefore a
type-level guarantee (the diff adds an unrelated method).

---

## 10. Open questions for the lead (gating plan-approval)

1. **Helper location** — confirmed lb-io per plan §3. OK to proceed?
2. **Error-variant collapse in the pool wrapper (§4)** — Phase 1 collapses
   `IdleSendError::{IdleTimeout, HeadTimeout}` onto the existing
   `Http2PoolError::Timeout` to keep the enum surface stable for
   `h3_bridge.rs:2573` and existing pool tests. Builder-1 (Phase 2) can
   request a phase-split variant when wiring the cells if they want
   phase-attribution at the lb-l7 boundary. Approve the collapse for
   Phase 1?
3. **Pool method name** — `send_request_idle` was chosen for grep-ability
   alongside `send_request`. Alternatives: `send_request_with_idle`,
   `send_request_progress_bounded`. Lead pref?
4. **`epoch` argument** — passing `tokio::time::Instant` through the public
   pool method API is unusual (most pool APIs hide their clock). The
   alternative is the helper internally calling `tokio::time::Instant::now()`
   at entry and treating that as the epoch — simpler API, but it forces the
   pump to pass `last_progress` in millis-since-its-OWN-epoch, which then
   must be ≤ helper-epoch to avoid an underflow. Current design (caller
   supplies epoch) is the safer composition; lead to confirm the API trade.
5. **`Send + Sync` bounds on the helper's `F`** — the pool method calls it
   from an `async fn` that may be polled across `.await` points in cell
   pumps that are `tokio::spawn`-ed onto a multi-thread runtime, so `F:
   Send` is required. The lb-l7 H1-client send_fut is `Send`; lb-io's
   `sender.send_request(...)` future is `Send`. Confirmed by reading hyper
   1.x — but flag as a Phase-2 compile-check gate.

I'll wait for explicit plan-approval (and answers to 1–5) before touching
any `.rs` file.

---

## 11. Memory hooks (for the auto-memory index)

- The helper is the **single-source** point for the matrix-wide
  CF-BODY-WALLCLOCK fix — future R12 audits should grep `idle_bounded_send`
  to confirm all 4 cells (post-Phase-2) reach the same loop body.
- The `epoch`-as-`tokio::time::Instant` choice is what makes the unit tests
  test-clock-deterministic — relevant if a future contributor swaps to
  `SystemTime` "because it's simpler", they'll break the paused-clock arms.
- The `biased;` in the select is load-bearing for arm (iv); a future
  refactor that removes it must justify how it preserves the
  "success-future-resolves-at-same-instant-as-timer" arm.
