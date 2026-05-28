//! S14 / CF-BODY-WALLCLOCK Phase 1 — single-sourced **two-phase
//! idle/head deadline** helper for request-egress send futures.
//!
//! Background ([[cf-body-wallclock]]): the H1/H2 proxy cells previously
//! bounded an opaque hyper `send_request` future with a fixed wall-clock
//! [`HttpTimeouts::body`], which 504-truncated slow-but-progressing
//! uploads (the same defect class F-S7-6 already fixed on the H3 connector
//! via [`H3_RESP_IDLE_TIMEOUT`]). [`idle_bounded_send`] replaces that
//! fixed wall-clock with a two-phase deadline:
//!
//! * **Phase A — upload-in-flight:** a NO-FORWARD-PROGRESS idle watchdog.
//!   The caller's request-egress pump bumps an `Arc<AtomicU64>`
//!   `last_progress` (millis since a [`tokio::time::Instant`] epoch) on
//!   every successful chunk hand-off into the bounded body channel — the
//!   same forward-progress event the R8 in-flight gauge already records.
//!   The deadline `last_progress + idle` is re-armed on every bump; it
//!   fires only when no bump arrives for `idle`, the L7 analogue of
//!   F-S7-6's `send_progress!` reset.
//!
//! * **Phase B — upload-complete:** the pump signals `upload_complete =
//!   true` exactly once at the terminal frame; the helper then switches
//!   to a fixed `head_timeout` cap on the remaining wait for the wrapped
//!   future's `Output` (the response HEAD). This special-case is the
//!   genuinely-hard part the lead flagged (plan §1.1): the opaque hyper
//!   send makes the post-upload head-wait un-idle-able from outside.
//!
//! The helper is generic over the wrapped future so BOTH Class A
//! (hyper H1-client `send_request`, called from lb-l7 directly) and
//! Class B (lb-io [`Http2Pool::send_request_idle`]) reuse the SAME loop
//! body — single-source per plan §3. The helper holds NO body bytes;
//! memory and backpressure invariants are unchanged (plan §4).
//!
//! [`HttpTimeouts::body`]: ../../../lb_l7/proxy/struct.HttpTimeouts.html
//! [`H3_RESP_IDLE_TIMEOUT`]: ../../../lb_quic/h3_bridge/constant.H3_RESP_IDLE_TIMEOUT.html
//! [`Http2Pool::send_request_idle`]: crate::http2_pool::Http2Pool::send_request_idle

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::Context;
use std::time::Duration;

use tokio::time::Instant;

/// Outcome of an [`idle_bounded_send`] timeout. The two-variant split
/// preserves the firing phase for caller-level logs/metrics; the pool
/// wrapper currently collapses both onto
/// [`crate::http2_pool::Http2PoolError::Timeout`] (Phase 1 stable-enum
/// constraint — see `Http2Pool::send_request_idle` doc).
#[derive(Debug, thiserror::Error)]
pub enum IdleSendError {
    /// **Phase A** wedge — no `tx.send` success for `idle` while the
    /// wrapped future remained pending. The carried [`Duration`] is the
    /// configured `idle` value.
    #[error("upload idle timeout: no forward progress for {0:?}")]
    IdleTimeout(Duration),
    /// **Phase B** wedge — the pump signalled `upload_complete = true`,
    /// but the wrapped future did not resolve within `head_timeout`. The
    /// carried [`Duration`] is the configured `head_timeout` value.
    #[error("head timeout: upload complete, no head for {0:?}")]
    HeadTimeout(Duration),
}

/// Drive a pinned send future under a two-phase idle/head deadline.
///
/// See the module docs for the mechanism and rationale. Concretely:
///
/// * `send_fut` — the opaque-from-outside send future (hyper H1-client
///   `send_request` for Class A; the pool's internal `send_request` for
///   Class B). Generic so both classes share this loop.
/// * `last_progress` — millis since `epoch`. Pump WRITER:
///   `last_progress.store(now_ms, Ordering::Relaxed)` after each
///   successful `tx.send`. Helper READER:
///   `last_progress.load(Ordering::Relaxed)` per watchdog tick.
/// * `upload_complete` — flipped exactly once by the pump at the terminal
///   frame, with `Ordering::Release`. Helper reads with
///   `Ordering::Acquire`, pairing so the helper sees the FINAL
///   `last_progress` bump before observing `upload_complete = true`.
/// * `epoch` — the [`tokio::time::Instant`] captured at request start;
///   `last_progress` values are millis-since-`epoch`. Using
///   [`tokio::time::Instant`] (not `SystemTime`) makes the helper
///   test-clock-friendly under `tokio::time::pause` / `start_paused`.
/// * `idle` — Phase A watchdog interval. Must be > 0.
/// * `head_timeout` — Phase B fixed cap. Must be > 0.
///
/// # Errors
///
/// * [`IdleSendError::IdleTimeout`] — Phase A no-progress wedge.
/// * [`IdleSendError::HeadTimeout`] — Phase B post-upload head wedge.
///
/// On success the wrapped future's `Output` is returned unchanged.
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
    // SAFETY rationale: we own `send_fut` by value, then immediately pin
    // it on the local stack via `tokio::pin!`. The pin is never moved
    // (the future is polled in-place via `Pin::as_mut()` inside the
    // loop's biased select), satisfying `Future`'s pinning contract for
    // a non-`Unpin` `F`.
    tokio::pin!(send_fut);

    // Anchor for Phase B: captured the FIRST tick the helper observes
    // `upload_complete == true`. Once set, it is never recomputed — a
    // slow head genuinely fires at `head_timeout` after the upload-done
    // observation, not idle.
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
            // `biased;` is load-bearing: when a ready send future and a
            // simultaneously-firing timer both resolve at the same
            // virtual instant (notably under `tokio::time::pause`),
            // success must win over a spurious timeout. Unit-test arm
            // (iv) (upload-complete then fast head resolving at the
            // deadline edge) depends on this.
            biased;

            out = poll_fn_send(&mut send_fut) => {
                return Ok(out);
            }
            () = tokio::time::sleep_until(deadline) => {
                if complete {
                    return Err(IdleSendError::HeadTimeout(head_timeout));
                }
                // Re-check: a pump bump may have landed AFTER we
                // computed `deadline` but BEFORE the timer expired
                // (a race the watchdog tick must absorb, plan §3).
                let lp_ms_now = last_progress.load(Ordering::Relaxed);
                let now = Instant::now();
                let last_progress_instant =
                    epoch + Duration::from_millis(lp_ms_now);
                if now.saturating_duration_since(last_progress_instant)
                    < idle
                {
                    // Progress landed; re-arm on the next iteration.
                    continue;
                }
                return Err(IdleSendError::IdleTimeout(idle));
            }
        }
    }
}

/// Polls a pinned future to completion as an async fn, allowing it to
/// participate in a `tokio::select!` arm by reference. Equivalent to the
/// `&mut send_fut` arm pattern but avoids the `Future` re-implementation
/// boilerplate at the call site.
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

    // (i) Chunk-by-chunk progress completes within idle → Ok(T).
    #[tokio::test(start_paused = true)]
    async fn arm_i_chunked_progress_completes() {
        let (last_progress, upload_complete, epoch) = fresh();
        let (tx, rx) = oneshot::channel::<u32>();

        // Pump task: bump every 500 ms (virtual) for 3 s, then complete +
        // resolve send_fut.
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
        // Total virtual time ≈ 3 s; well within head_timeout.
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
        // Should fire at ≈ 1 s (the idle); allow small slack for
        // re-check loop iteration.
        assert!(
            elapsed >= Duration::from_secs(1) && elapsed < Duration::from_millis(1_500),
            "fire instant out of band: {elapsed:?}",
        );
    }

    // (iii) Upload-complete then slow head → Err(HeadTimeout) at
    // head_timeout, NOT idle. THIS IS THE LOAD-BEARING TWO-PHASE PROOF.
    #[tokio::test(start_paused = true)]
    async fn arm_iii_complete_then_slow_head_fires_head() {
        let (last_progress, upload_complete, epoch) = fresh();
        let never = std::future::pending::<u32>();

        // Pump: bump at t=100 ms, complete at t=200 ms; future never
        // resolves. With idle = 500 ms and head_timeout = 5 s, a
        // single-phase idle watchdog would fire at t=600 ms. The two-
        // phase helper must fire at t ≈ 200 ms + 5 s = 5200 ms instead.
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
        // Must NOT have fired at the idle deadline (≈600 ms).
        assert!(
            elapsed > Duration::from_secs(1),
            "fired too early — idle, not head: {elapsed:?}",
        );
        // Must fire near 200 ms + 5 s = 5200 ms (upper bound widened
        // for the loop iteration that observes `complete` and then
        // anchors `now() + head_timeout`; the anchor is therefore set
        // at ~500 ms (next idle tick after the upload-complete flip),
        // not at exactly 200 ms, yielding a fire at ~5500 ms).
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

    // (v) Zero-bump (immediate wedge) with a different `idle` value —
    // asserts the deadline scales with the parameter rather than firing
    // on a hardcoded epoch.
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

    // (vi) Late bump re-arms the watchdog (regression guard for the
    // `continue` path).
    #[tokio::test(start_paused = true)]
    async fn arm_vi_late_bump_rearms() {
        let (last_progress, upload_complete, epoch) = fresh();
        let never = std::future::pending::<u32>();

        // Bump at t=0 (already 0 in last_progress), then at t=400 ms
        // (before the t=500 ms idle fire), then never again. Fire must
        // land at ≈ 400 + 500 = 900 ms, NOT 500 ms.
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

    // (vii) Bump lands AT the deadline tick (race re-check): the helper
    // re-loads `last_progress` after the timer fires and re-arms if a
    // bump landed in the gap. This exercises the explicit re-check
    // branch in §3 of the design.
    #[tokio::test(start_paused = true)]
    async fn arm_vii_tick_race_recheck() {
        let (last_progress, upload_complete, epoch) = fresh();
        let never = std::future::pending::<u32>();

        // Bump at t=499 ms; the t=500 ms timer fires, the re-check sees
        // an effective gap of ≈ 1 ms < 500 ms, re-arms; next fire lands
        // at ≈ 499 + 500 = 999 ms.
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
}
