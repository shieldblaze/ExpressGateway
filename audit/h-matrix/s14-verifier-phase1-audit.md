# S14 Phase 1 verifier audit — INDEPENDENT

> **Author:** verifier (`feature/cfbw-s14-verify`, tip `e33343cd`).
> **Scope:** independent audit of pool-eng's Phase 1 deliverable
> (`crates/lb-io/src/idle_send.rs` + `Http2Pool::send_request_idle`).
> R5: author ≠ verifier — I did not look at pool-eng's claims; I re-ran
> and re-read.

---

## 1. Code-correctness audit — read top-to-bottom

### 1.1 Helper module `crates/lb-io/src/idle_send.rs`

Module skeleton:
- `IdleSendError` — two variants (`IdleTimeout(Duration)`, `HeadTimeout(Duration)`),
  `#[derive(Debug, thiserror::Error)]`. Matches plan §5.
- `idle_bounded_send<F, T>(send_fut, last_progress, upload_complete, epoch, idle, head_timeout)
  -> Result<T, IdleSendError>` where `F: Future<Output = T>`.
- `poll_fn_send` helper polling `&mut Pin<&mut F>` via `std::future::poll_fn`.

Key audit points (lead-flagged):

#### (a) `biased;` arm — does success win at same virtual instant?

`tokio::select!` with `biased;` polls arms in source order on each tick. The first
arm is `out = poll_fn_send(&mut send_fut)`, the second is the timer. So a
ready send-future is polled FIRST: if it returns `Poll::Ready(out)`, the
select returns success without polling the timer arm. Even if both would
resolve at the exact same virtual instant under `tokio::time::pause`,
biased ordering picks the success arm. ✓

Arm (iv) under `start_paused = true` exercises this: pump bumps at 100 ms,
flips `upload_complete = true` at 200 ms, send_fut resolves at 300 ms.
Helper returns `Ok(7)`. Test passes ×3.

**Verdict: correct.**

#### (b) Lead-flagged race: `upload_complete` flipped between top-of-iter and timer firing — does helper incorrectly fire `IdleTimeout`?

Walked through several scenarios:

1. **Arm (iii) scenario** (bump@100 ms, complete@200 ms, never resolves):
   - Iter 1 top: complete (Acquire)=false, lp=0 → deadline=epoch+0+500=500 ms.
   - During sleep: pump bumps lp=100 at t=100, flips complete=true at t=200.
   - Timer fires at 500 ms. Branch: `complete` (captured top-of-iter)=false → re-check.
   - Re-check: lp_ms_now=100; now-lp_instant=400 ms < idle(500 ms) → `continue`.
   - Iter 2 top: complete (Acquire)=true → anchor = now()+head_timeout = 500+5000 = 5500 ms.
   - Timer fires at 5500 ms; complete=true → `HeadTimeout(5s)`. ✓
   - Empirical fire instant in [5000, 6000) ms (arm-(iii) assertion). PASS ×3.

2. **Pathological "bump-then-complete-no-final-bump"** (constructed): if pump bumps
   at t=0 and flips complete at t=499 with NO further bump and NO terminal-frame
   bump, idle=500 ms, head_timeout=5 s:
   - Iter 1: complete=false, deadline=500 ms.
   - Timer fires at 500 ms; complete (captured)=false → re-check.
   - lp_ms_now=0, now-lp=500 ms; `500 < 500` is false → `IdleTimeout(500 ms)`.
   - This WOULD be a misfire. But it requires `upload_complete=true` to be set
     WITHOUT a paired final `last_progress.store` — which the helper's documented
     pump contract (lines 73-79) explicitly forbids: every successful `tx.send`
     bumps `last_progress`, and the terminal frame is itself a `tx.send`, so the
     last bump always lands within the helper's idle window of the complete flip.
   - The Acquire/Release pairing further guarantees the helper sees the final
     bump before observing `complete=true`.

3. **Empty-body request** (no chunks; pump initializes `upload_complete=true`
   from the start): iter 1 top sees complete=true → enters Phase B immediately.
   No race. (Pool-arm `send_request_idle_success_arm` exercises this path; PASS.)

**Verdict on race I: in-spec under the documented pump contract. Helper is
correct.** The arm (iii) assertion range [5000, 6000) ms WOULD trap a misfire
(it would fire at ~600 ms instead). **Note:** the race is "in-spec" but
proof depends on pump-side discipline (always bump before flip). If a Phase-2
cell author flips `upload_complete` without a paired terminal-bump, the helper
could misfire. **Recommend:** add a unit-test arm in Phase 2 builder-1 (or
Phase 3 verifier) that exercises "complete-without-final-bump" to lock in
the pump contract, OR re-load `upload_complete` in the timer's re-check
branch to defang the contract dependency. Non-blocking suggestion; not a
defect for Phase 1 promote.

#### (c) Memory ordering

| Site | Variable | Ordering | Verdict |
|---|---|---|---|
| pump write | `last_progress.store` | Relaxed | ✓ Pump-side timestamp; no other variable ordered against it. Multi-writer-safe because helper only reads "latest best-effort". |
| pump write | `upload_complete.store(true)` | Release | ✓ Pairs with helper's Acquire load; ensures final `last_progress` bump visible before helper sees `complete=true`. |
| helper read | `last_progress.load` | Relaxed | ✓ Best-effort tick. |
| helper read | `upload_complete.load` | Acquire | ✓ Pairs with pump Release. |

**Verdict: correct.**

#### (d) `poll_fn_send` indirection soundness

```rust
async fn poll_fn_send<F: Future>(fut: &mut Pin<&mut F>) -> F::Output {
    std::future::poll_fn(|cx| fut.as_mut().poll(cx)).await
}
```

`fut: &mut Pin<&mut F>` — caller passes `&mut send_fut` where `send_fut` is
already stack-pinned via `tokio::pin!`. `Pin::as_mut(self: &mut Pin<P>) -> Pin<&mut P::Target>`
gives us `Pin<&mut F>` — the right shape for `Future::poll`. The closure
captures `fut` by mutable reference; the poll_fn future is awaited immediately
inside the select arm and is dropped after each select iteration. The outer
stack-pinned `send_fut` lives across iterations because it is on the
helper's own stack frame and never moved (Pin contract preserved). No UB. ✓

**Verdict: sound.**

#### (e) Body bytes — does helper hold any?

No `Bytes`, `Vec<u8>`, or chunk storage anywhere in `idle_send.rs`. The helper
holds: two `Arc<Atomic*>`, an `Instant`, two `Duration`s, one
`Option<Instant>` (head anchor), and the moved-in `send_fut`. The `send_fut`
carries the request body (it's the hyper send future), so body bytes flow
through hyper's machinery; the helper does NOT duplicate or buffer them.
R8 invariant (plan §4) preserved. ✓

### 1.2 `Http2Pool::send_request_idle` method (`crates/lb-io/src/http2_pool.rs:296-340`)

Structure-diff against `send_request`:
- Same `acquire_sender(addr).await?` (returns `Http2PoolError::Dial` /
  `Http2PoolError::Handshake` unchanged).
- Same `sender.send_request(request)` futures construction.
- Replaces `tokio::time::timeout(send_timeout, send_fut).await` with
  `idle_bounded_send(send_fut, …).await`.
- Nested-Result matching mirrors the original (hyper send returns
  `Result<Response, hyper::Error>` and helper wraps it in
  `Result<_, IdleSendError>`).
- Same eviction policy: `self.evict(addr)` on both `Send` error AND `Timeout`
  arms — preserves ROUND8-L7-10 H2-multiplex corruption guard.
- New: `tracing::warn!(phase=…, %addr, error=…, "h2 idle/head deadline fired")`
  on the timeout arm. Phase 1 collapses both `IdleSendError` variants onto
  `Http2PoolError::Timeout` with the discriminant logged. Enum surface
  unchanged → callers (`h3_bridge.rs:2573`, existing pool tests) untouched.
- `#[allow(clippy::too_many_arguments)]` is justified (8 args, all
  load-bearing per plan §1.3).

**Verdict: correct, byte-identical eviction semantics with `send_request`.**

### 1.3 `crates/lb-io/src/lib.rs` re-export

Line 26: `pub mod idle_send;` — present, no glob re-export. Callers go through
`lb_io::idle_send::{idle_bounded_send, IdleSendError}`. Matches plan §1.1.

### 1.4 Existing surface unchanged

- `Http2PoolConfig` — no new field.
- `Http2PoolError` — no new variant.
- `Http2Pool::send_request` — byte-identical (no edits).
- `acquire_sender` / `take_alive_sender` / `replace_entry` / `evict` /
  `dial_and_handshake` / `reset_peer` — untouched.

**Code-correctness verdict: APPROVE.**

---

## 2. Determinism re-run — ×3 `cargo test -p lb-io --all-features`

| Run | Pass | Fail | Filename |
|---|---|---|---|
| 1 | 55 | 0 | `audit/h-matrix/s14-verifier-phase1-1.log` |
| 2 | 55 | 0 | `audit/h-matrix/s14-verifier-phase1-2.log` |
| 3 | 55 | 0 | `audit/h-matrix/s14-verifier-phase1-3.log` |

Test-name sets diffed across the three runs (`diff /tmp/s14p1-r{1,2,3}.txt`):
**identical**.

idle_send test arms present in all 3 runs:
- `arm_i_chunked_progress_completes`
- `arm_ii_immediate_wedge_idle`
- `arm_iii_complete_then_slow_head_fires_head`
- `arm_iv_complete_then_fast_head_succeeds`
- `arm_v_zero_bump_scaled_idle`
- `arm_vi_late_bump_rearms`
- `arm_vii_tick_race_recheck`

Pool arms (`http2_pool::tests`):
- `send_request_idle_dial_fail_smoke`
- `send_request_idle_success_arm`
- `send_request_idle_head_timeout_arm`
- `send_request_idle_idle_timeout_arm`

**Verdict: APPROVE — deterministic 55/55 ×3.**

---

## 3. Coverage re-measure — INDEPENDENT scoped llvm-cov

_Section to be filled in once llvm-cov workspace run completes._

---

## 4. R3 regression check — full workspace

_Section to be filled in once `cargo test --workspace --all-features` completes._

---

## 5. Overall verdict

_Final summary to be appended after sections 3-4._
