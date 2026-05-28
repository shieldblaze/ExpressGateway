# S14 Phase 3 — VERIFIER ESCALATION (R14): CF-BODY-WALLCLOCK helper race-on-small-body

**Status:** DEFECT, repro reproduced 3× in isolation, escalating to lead per R14.
**Worktree tip:** `feature/cfbw-s14-verify` rebased on `e83cf47e`.
**Cell affected:** H1→H1 (`crates/lb-l7/src/h1_proxy.rs:1572` calls
`lb_io::idle_send::idle_bounded_send`). Branch-equivalence implies all 4
CF-BODY-WALLCLOCK cells are affected.

## What I expected (per R13 arm (c))

A request with a small body that the backend drains successfully, then
hangs without replying, should fire `head_timeout` (Phase B), not
`body` (Phase A). Setup: `HttpTimeouts { body = 500 ms, head = 10 s }`,
4-byte `Content-Length: 4` request body, backend drains then `pending()`.

Expected: gateway 504s at ~10 s (head).
Observed: gateway 504s at **501 ms (body)** — Phase B is never reached.

## Repro (deterministic, 3/3 across body values 30s/10s/5s/500ms)

```rust
let to = HttpTimeouts {
    header: Duration::from_secs(30),
    body: Duration::from_millis(500),
    total: Duration::from_secs(60),
    head: Duration::from_secs(10),
};
// 4-byte CL=4 POST → drain-then-pending backend → measure 504 elapsed.
```

Observed elapsed at body values: 30 s → 30.00 s; 10 s → 10.00 s;
5 s → 5.00 s; 500 ms → 501 ms. All match `body` exactly. None match `head`.

## Root cause (proven mechanism)

`crates/lb-io/src/idle_send.rs:139-157` — the timer-fired branch reads the
LOCAL `complete` captured at top-of-iter, NOT a fresh `upload_complete`
load:

```rust
() = tokio::time::sleep_until(deadline) => {
    if complete {  // STALE: captured at line 118 at top of iter
        return Err(IdleSendError::HeadTimeout(head_timeout));
    }
    // Re-check: a pump bump may have landed ... (but NOT upload_complete!)
    let lp_ms_now = last_progress.load(Ordering::Relaxed);
    let now = Instant::now();
    let last_progress_instant = epoch + Duration::from_millis(lp_ms_now);
    if now.saturating_duration_since(last_progress_instant) < idle {
        continue;
    }
    return Err(IdleSendError::IdleTimeout(idle));
}
```

For a small request body, the pump's path:
1. `body.frame().await` returns the 4-byte data frame (lp_ms ≈ 0 because
   `tokio::time::Instant::now()` minus the just-captured `epoch` rounds to 0).
2. `tx.send(...).await` succeeds once hyper polls the request body.
3. `bump()` runs: `last_progress.store(0, Relaxed)` (still ≈ 0).
4. Next `body.frame().await` returns None → `set_complete()` runs:
   `upload_complete.store(true, Release)`.

The helper's iter 1 captured `complete = false` (Acquire load) at iter
start BEFORE the pump's `set_complete` could land (race window: from
`epoch + 0` to whenever the pump's task is scheduled). Helper then enters
its select arm and sleeps until `deadline = epoch + 0 + body`. Backend
hangs forever; send_fut never resolves. Timer fires at `t = body`.

Re-check arithmetic:
- `lp_ms_now = 0` (the only bump landed at lp = 0).
- `now - last_progress_instant = (epoch + body) - (epoch + 0) = body`.
- `body < body` is **FALSE** → fall through to `return IdleSendError::IdleTimeout`.

The helper NEVER re-reads `upload_complete` after the timer fires, so it
cannot recover even though `complete = true` was set ≤ a few ms after
iter 1's load. Phase B is unreachable for this scenario.

## Why the Phase 1 audit's "in-spec under pump contract" verdict missed this

Phase 1 audit §1(b) walked through the bump-then-complete-no-final-bump
case and ruled it "in-spec because the pump contract requires a final
bump immediately before flipping complete". That is satisfied here (the
4-byte data frame's bump happens BEFORE set_complete). The defect is NOT
"no bump before complete"; it is "bump and complete both land at
`lp_ms ≈ 0` (within tokio millisecond resolution of `epoch`), and the
strict-less-than comparison in the re-check (`diff < idle`) is `FALSE`
at exactly `diff == idle`". I should have flagged this in Phase 1 — my
"low priority strengthening suggestion" should have been ESCALATE.
Lesson logged for [[s2-verification-gap]]-class verification gap.

## Test arm (iii) does not trap this

Arm (iii) (`crates/lb-io/src/idle_send.rs:266`) uses `idle = 500 ms`,
`head_timeout = 5 s`, bump at virtual t=100 ms, complete at virtual
t=200 ms, expects fire in [5000, 6000) ms. Walking through:
- Iter 1 top (t=0): complete=false, lp=0, deadline = epoch + 0 + 500 = 500 ms.
- Sleep until 500 ms. During sleep: bump (lp=100) at 100 ms, complete
  flips at 200 ms.
- Timer fires at 500 ms. complete (captured) = false → re-check.
- lp_ms_now = 100. `now - last_progress_instant = 500 - 100 = 400 ms`.
- `400 ms < 500 ms (idle)` → TRUE → `continue`.
- Iter 2 top: complete=true → anchor B = 500 + 5000 = 5500 ms → fires.

Arm (iii) works because the bump (lp=100) is far enough BEHIND the timer
(`now=500`) that `diff=400ms < idle=500ms` is true and the re-check
re-arms. The defect is **only** visible when the last bump lands at
`lp_ms ≈ 0` (small body, immediate frame, sub-ms scheduling). Arm (iii)
does NOT have a bump-at-0 variant.

## Repro at `idle_send.rs` (helper-isolation, no cell wiring)

I have NOT authored a helper-isolation arm yet — the cell-level test in
`tests/s14_cfbw_h1h1.rs` (arm c) reproduces it through the full proxy,
which is sufficient evidence. A helper-isolation arm would be:

```rust
// PROPOSED arm (ix) — small-body race: lp=0 bump + complete-just-after.
#[tokio::test(start_paused = true)]
async fn arm_ix_lp_zero_bump_then_complete_fires_head_not_idle() {
    let (last_progress, upload_complete, epoch) = fresh();
    let never = std::future::pending::<u32>();
    let lp = last_progress.clone();
    let uc = upload_complete.clone();
    tokio::spawn(async move {
        // Bump at t=0 ms (lp stays 0), complete at t=1 ms.
        bump_to_now(&lp, epoch);
        tokio::time::sleep(Duration::from_millis(1)).await;
        uc.store(true, Ordering::Release);
    });
    let res = idle_bounded_send(
        never, last_progress, upload_complete, epoch,
        Duration::from_millis(500),  // idle
        Duration::from_secs(5),       // head
    ).await;
    assert!(
        matches!(res, Err(IdleSendError::HeadTimeout(d)) if d == Duration::from_secs(5)),
        "expected HeadTimeout(5s) — bump-at-0 + complete-just-after must NOT misfire as IdleTimeout; got {res:?}",
    );
}
```

I expect this PROPOSED arm to FAIL on the current helper (mirroring the
cell-level repro), confirming the helper-level defect.

## Suggested fix (1-line, in lb-io — pool-eng's territory)

In the timer-fired branch, re-load `upload_complete` instead of using
the stale captured value:

```rust
() = tokio::time::sleep_until(deadline) => {
    // S14 FIX: re-load upload_complete; it may have flipped during sleep.
    let complete_now = upload_complete.load(Ordering::Acquire);
    if complete_now {
        // Phase B has been reached; re-anchor or fall through to the
        // next iter (which will set the anchor). Returning HeadTimeout
        // here would be wrong since we haven't waited head_timeout yet.
        continue;
    }
    // ... re-check last_progress as before ...
}
```

This converts a misfire into a re-arm onto Phase B (correct behavior).
The next iter's deadline becomes `Instant::now() + head_timeout`.

Alternative: use `<=` instead of `<` in the bump re-check:
`if now.saturating_duration_since(last_progress_instant) <= idle` —
absorbs the lp≈0 case as a re-arm. Less robust (still doesn't handle
the case where lp is actually stale and complete flipped → re-arms onto
Phase A, then re-iters, then sees complete and goes to Phase B). The
re-load fix is cleaner.

## Impact

- All 4 cells are affected (R12 equivalence): H1→H1, H2→H1 B-B, H1→H2,
  H2→H2. Any small-body request that elicits a slow response head will
  504 at `body` instead of `head`. Pre-Phase-2 these requests 504'd at
  `body` too (wall-clock), so this is NOT a regression on the small-body
  case — it's the head_timeout knob being silently unreachable for
  small bodies. The slow-progressing-LARGE-body case (arm a) IS fixed
  correctly (verified by `s14_cfbw_h1h1::cfbw_a_slow_progressing_upload_succeeds`
  PASS).

- The `HttpTimeouts::head` knob behavior is therefore: head_timeout
  works iff `lp_ms ≥ ε` at the time `upload_complete` flips, i.e. only
  for bodies that took non-trivial wall-clock time to stream
  (per-chunk pacing ≥ ~10 ms, or large enough total bytes). Smaller
  bodies silently use `body` as the head cap, which is the OLD
  wall-clock behavior pool-eng's design explicitly aimed to fix.

## R14 verdict

CLOSE-BLOCKER for Phase 3 promote. Suggest pool-eng implements the
helper-level fix (re-load upload_complete in the timer branch +
proposed arm (ix) added to `idle_send::tests`); builder-1 re-verifies
no cell-wiring change needed; verifier re-runs Phase 3 R13 cell tests +
R12 equivalence + ×3 gate + cov.

## What I will do next

1. DM lead with this finding + STOP Phase 3 work pending the fix.
2. Hold the verifier-authored `tests/s14_cfbw_h1h1.rs` arms (a/b/c +
   control) un-committed so they re-run cleanly against the fixed
   helper.
3. Do NOT author the other 3 cell test files yet — they would all hit
   the same defect and waste cycles.
