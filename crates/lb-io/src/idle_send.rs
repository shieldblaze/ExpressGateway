//! Two-phase idle/head deadline for request-egress send futures (CF-BODY-WALLCLOCK).
//!
//! A fixed wall-clock cap on an opaque hyper `send_request` 504-truncates slow-but-PROGRESSING
//! uploads. So Phase A watches for no-forward-progress instead (re-armed on every pump bump), and
//! Phase B switches to a fixed `head_timeout` once the upload is done — the post-upload head-wait
//! cannot be idle-watched from outside the opaque send.
//!
//! Generic over the wrapped future so the H1-client and H2-pool paths share ONE loop body.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::Context;
use std::time::Duration;

use tokio::time::Instant;

/// Which phase timed out. Split so callers can log it, though the pool wrapper collapses both
/// onto [`crate::http2_pool::Http2PoolError::Timeout`].
#[derive(Debug, thiserror::Error)]
pub enum IdleSendError {
    /// Phase A: no forward progress for the carried `idle` duration.
    #[error("upload idle timeout: no forward progress for {0:?}")]
    IdleTimeout(Duration),
    /// Phase B: upload complete, but no head within the carried `head_timeout`.
    #[error("head timeout: upload complete, no head for {0:?}")]
    HeadTimeout(Duration),
}

/// Drive `send_fut` under the two-phase deadline; `idle` and `head_timeout` must both be > 0.
///
/// Caller contract for the two atomics: the pump stores millis-since-`epoch` into
/// `last_progress` (Relaxed) after each successful `tx.send`, and flips `upload_complete` exactly
/// once at the terminal frame with `Release`. The helper's `Acquire` load pairs with that flip so
/// it always sees the FINAL progress bump before observing completion.
///
/// `epoch` is a [`tokio::time::Instant`], not `SystemTime`, so tests can drive it under
/// `tokio::time::pause`.
///
/// # Errors
///
/// [`IdleSendError`]. On success the wrapped future's `Output` passes through unchanged.
pub async fn idle_bounded_send<F, T>(
    send_fut: F,
    last_progress: Arc<AtomicU64>,
    upload_complete: Arc<AtomicBool>,
    epoch: Instant,
    idle: Duration,
    head_timeout: Duration,
) -> Result<T, IdleSendError>
where
    F: Future<Output = T>,
{
    // Owned by value then pinned in place and only ever polled via `Pin::as_mut()`, which is what
    // satisfies the pinning contract for a non-`Unpin` `F`.
    tokio::pin!(send_fut);

    // Set on the FIRST tick that observes completion and never recomputed, so a slow head fires
    // at `head_timeout` past the upload-done observation rather than sliding.
    let mut head_deadline_anchor: Option<Instant> = None;

    loop {
        let complete = upload_complete.load(Ordering::Acquire);

        let deadline: Instant = if complete {
            *head_deadline_anchor.get_or_insert_with(|| Instant::now() + head_timeout)
        } else {
            let lp_ms = last_progress.load(Ordering::Relaxed);
            epoch + Duration::from_millis(lp_ms) + idle
        };

        tokio::select! {
            // Load-bearing: when the send future and the timer resolve at the same virtual
            // instant, success MUST win over a spurious timeout. Arm (iv) depends on it.
            biased;

            out = poll_fn_send(&mut send_fut) => {
                return Ok(out);
            }
            () = tokio::time::sleep_until(deadline) => {
                if complete {
                    return Err(IdleSendError::HeadTimeout(head_timeout));
                }
                // MUST re-load: `complete` from the top of the iteration may be stale after the
                // sleep. Without this re-load a small body whose bump lands at `lp_ms ≈ 0` with
                // the completion flip just after misfires IdleTimeout, making Phase B — and so
                // `head_timeout` — silently unreachable for small bodies (S14 CFBW-RECHECK).
                if upload_complete.load(Ordering::Acquire) {
                    continue;
                }
                // A pump bump may have landed after `deadline` was computed but before it fired.
                let lp_ms_now = last_progress.load(Ordering::Relaxed);
                let now = Instant::now();
                let last_progress_instant =
                    epoch + Duration::from_millis(lp_ms_now);
                if now.saturating_duration_since(last_progress_instant)
                    < idle
                {
                    continue;
                }
                return Err(IdleSendError::IdleTimeout(idle));
            }
        }
    }
}

/// Polls a pinned future by reference so it can sit in a `select!` arm without being consumed.
async fn poll_fn_send<F: Future>(fut: &mut Pin<&mut F>) -> F::Output {
    std::future::poll_fn(|cx: &mut Context<'_>| fut.as_mut().poll(cx)).await
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation
)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use tokio::sync::oneshot;

    fn fresh() -> (Arc<AtomicU64>, Arc<AtomicBool>, Instant) {
        (
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicBool::new(false)),
            Instant::now(),
        )
    }

    fn bump_to_now(last_progress: &Arc<AtomicU64>, epoch: Instant) {
        let dt = Instant::now().saturating_duration_since(epoch);
        let ms = u64::try_from(dt.as_millis()).unwrap_or(u64::MAX);
        last_progress.store(ms, Ordering::Relaxed);
    }

    // (i) Chunk-by-chunk progress completes within idle.
    #[tokio::test(start_paused = true)]
    async fn arm_i_chunked_progress_completes() {
        let (last_progress, upload_complete, epoch) = fresh();
        let (tx, rx) = oneshot::channel::<u32>();

        let lp = last_progress.clone();
        let uc = upload_complete.clone();
        let ep = epoch;
        tokio::spawn(async move {
            for _ in 0..6 {
                tokio::time::sleep(Duration::from_millis(500)).await;
                bump_to_now(&lp, ep);
            }
            uc.store(true, Ordering::Release);
            let _ = tx.send(42);
        });

        let res = idle_bounded_send(
            async move { rx.await.unwrap() },
            last_progress,
            upload_complete,
            epoch,
            Duration::from_secs(1),
            Duration::from_secs(5),
        )
        .await;

        assert!(matches!(res, Ok(42)), "got {res:?}");
        let elapsed = Instant::now().saturating_duration_since(epoch);
        assert!(
            elapsed < Duration::from_secs(5),
            "elapsed too large: {elapsed:?}",
        );
    }

    // (ii) Wedge from the start → Err(IdleTimeout) at idle.
    #[tokio::test(start_paused = true)]
    async fn arm_ii_immediate_wedge_idle() {
        let (last_progress, upload_complete, epoch) = fresh();
        let never = std::future::pending::<u32>();

        let res = idle_bounded_send(
            never,
            last_progress,
            upload_complete,
            epoch,
            Duration::from_secs(1),
            Duration::from_secs(5),
        )
        .await;

        assert!(
            matches!(res, Err(IdleSendError::IdleTimeout(d)) if d == Duration::from_secs(1)),
            "got {res:?}",
        );
        let elapsed = Instant::now().saturating_duration_since(epoch);
        assert!(
            elapsed >= Duration::from_secs(1) && elapsed < Duration::from_millis(1_500),
            "fire instant out of band: {elapsed:?}",
        );
    }

    // (iii) THE two-phase proof: complete-then-slow-head must fire HeadTimeout, not Idle.
    #[tokio::test(start_paused = true)]
    async fn arm_iii_complete_then_slow_head_fires_head() {
        let (last_progress, upload_complete, epoch) = fresh();
        let never = std::future::pending::<u32>();

        // A single-phase idle watchdog would fire at t≈600 ms; the two-phase helper must not.
        let lp = last_progress.clone();
        let uc = upload_complete.clone();
        let ep = epoch;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            bump_to_now(&lp, ep);
            tokio::time::sleep(Duration::from_millis(100)).await;
            uc.store(true, Ordering::Release);
        });

        let res = idle_bounded_send(
            never,
            last_progress,
            upload_complete,
            epoch,
            Duration::from_millis(500),
            Duration::from_secs(5),
        )
        .await;

        assert!(
            matches!(res, Err(IdleSendError::HeadTimeout(d)) if d == Duration::from_secs(5)),
            "got {res:?} (expected HeadTimeout(5s) — two-phase regression)",
        );
        let elapsed = Instant::now().saturating_duration_since(epoch);
        assert!(
            elapsed > Duration::from_secs(1),
            "fired too early — idle, not head: {elapsed:?}",
        );
        // The anchor is set on the idle tick AFTER the flip (~500 ms), not at the flip itself,
        // so the fire lands near 5500 ms rather than 5200 ms.
        assert!(
            elapsed >= Duration::from_millis(5_000) && elapsed < Duration::from_millis(6_000),
            "head fire instant out of band: {elapsed:?}",
        );
    }

    // (iv) Upload-complete then fast head → Ok(T).
    #[tokio::test(start_paused = true)]
    async fn arm_iv_complete_then_fast_head_succeeds() {
        let (last_progress, upload_complete, epoch) = fresh();
        let (tx, rx) = oneshot::channel::<u32>();

        let lp = last_progress.clone();
        let uc = upload_complete.clone();
        let ep = epoch;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            bump_to_now(&lp, ep);
            tokio::time::sleep(Duration::from_millis(100)).await;
            uc.store(true, Ordering::Release);
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = tx.send(7);
        });

        let res = idle_bounded_send(
            async move { rx.await.unwrap() },
            last_progress,
            upload_complete,
            epoch,
            Duration::from_millis(500),
            Duration::from_secs(5),
        )
        .await;

        assert!(matches!(res, Ok(7)), "got {res:?}");
    }

    // (v) A different `idle` must scale the deadline, not fire off a hardcoded epoch.
    #[tokio::test(start_paused = true)]
    async fn arm_v_zero_bump_scaled_idle() {
        let (last_progress, upload_complete, epoch) = fresh();
        let never = std::future::pending::<u32>();

        let res = idle_bounded_send(
            never,
            last_progress,
            upload_complete,
            epoch,
            Duration::from_millis(750),
            Duration::from_secs(5),
        )
        .await;

        assert!(
            matches!(res, Err(IdleSendError::IdleTimeout(d)) if d == Duration::from_millis(750)),
            "got {res:?}",
        );
        let elapsed = Instant::now().saturating_duration_since(epoch);
        assert!(
            elapsed >= Duration::from_millis(750) && elapsed < Duration::from_millis(1_250),
            "fire instant out of band: {elapsed:?}",
        );
    }

    // (vi) Regression guard for the re-arm `continue` path.
    #[tokio::test(start_paused = true)]
    async fn arm_vi_late_bump_rearms() {
        let (last_progress, upload_complete, epoch) = fresh();
        let never = std::future::pending::<u32>();

        // A bump at t=400 ms must push the fire to ~900 ms, not 500 ms.
        let lp = last_progress.clone();
        let ep = epoch;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(400)).await;
            bump_to_now(&lp, ep);
        });

        let res = idle_bounded_send(
            never,
            last_progress,
            upload_complete,
            epoch,
            Duration::from_millis(500),
            Duration::from_secs(5),
        )
        .await;

        assert!(
            matches!(res, Err(IdleSendError::IdleTimeout(_))),
            "got {res:?}"
        );
        let elapsed = Instant::now().saturating_duration_since(epoch);
        assert!(
            elapsed >= Duration::from_millis(900) && elapsed < Duration::from_millis(1_400),
            "re-arm fire instant out of band: {elapsed:?}",
        );
    }

    // (vii) A bump landing in the timer gap must be absorbed by the post-fire re-check.
    #[tokio::test(start_paused = true)]
    async fn arm_vii_tick_race_recheck() {
        let (last_progress, upload_complete, epoch) = fresh();
        let never = std::future::pending::<u32>();

        let lp = last_progress.clone();
        let ep = epoch;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(499)).await;
            bump_to_now(&lp, ep);
        });

        let res = idle_bounded_send(
            never,
            last_progress,
            upload_complete,
            epoch,
            Duration::from_millis(500),
            Duration::from_secs(5),
        )
        .await;

        assert!(
            matches!(res, Err(IdleSendError::IdleTimeout(_))),
            "got {res:?}"
        );
        let elapsed = Instant::now().saturating_duration_since(epoch);
        assert!(
            elapsed >= Duration::from_millis(950) && elapsed < Duration::from_millis(1_500),
            "tick-race re-check fire out of band: {elapsed:?}",
        );
    }

    // (ix) Non-vacuity proof for the CFBW-RECHECK re-load: FAILS pre-fix (IdleTimeout at
    // t≈500 ms), PASSES post-fix (HeadTimeout at t≈5500 ms).
    #[tokio::test(start_paused = true)]
    async fn arm_ix_lp_zero_bump_then_complete_fires_head_not_idle() {
        let (last_progress, upload_complete, epoch) = fresh();
        let never = std::future::pending::<u32>();

        // Bumping immediately stores lp_ms == 0; the flip follows at t=1 ms.
        let lp = last_progress.clone();
        let uc = upload_complete.clone();
        let ep = epoch;
        tokio::spawn(async move {
            bump_to_now(&lp, ep);
            tokio::time::sleep(Duration::from_millis(1)).await;
            uc.store(true, Ordering::Release);
        });

        let res = idle_bounded_send(
            never,
            last_progress,
            upload_complete,
            epoch,
            Duration::from_millis(500), // idle
            Duration::from_secs(5),     // head
        )
        .await;

        assert!(
            matches!(res, Err(IdleSendError::HeadTimeout(d)) if d == Duration::from_secs(5)),
            "lp=0 + complete-just-after must fire HeadTimeout(5s) post-fix; \
             got {res:?} (pre-fix: IdleTimeout(500ms))",
        );
        let elapsed = Instant::now().saturating_duration_since(epoch);
        assert!(
            elapsed > Duration::from_secs(1),
            "fired too early — re-load fix not load-bearing: {elapsed:?}",
        );
        assert!(
            elapsed >= Duration::from_millis(5_000) && elapsed < Duration::from_millis(6_500),
            "head-fire instant out of band: {elapsed:?}",
        );
    }
}
