//! WebSocket backpressure management.
//!
//! Pauses client reads when the backend is not writable and resumes when the
//! backend drains. Uses a `Notify` for efficient async wakeup instead of polling.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Notify;

/// Tracks backpressure state between client and backend WebSocket connections.
///
/// Both halves of the proxy share a clone of this state. When the backend
/// write buffer fills, the writing side calls [`pause`] to signal the reading
/// side to stop pulling frames. When the buffer drains, [`resume`] unblocks
/// the reader via an async notification (no busy-spin).
#[derive(Debug, Clone)]
pub struct BackpressureState {
    inner: Arc<BackpressureInner>,
}

#[derive(Debug)]
struct BackpressureInner {
    /// When `true`, the client side should pause reading frames.
    paused: AtomicBool,
    /// Notified when transitioning from paused to resumed.
    notify: Notify,
}

impl BackpressureState {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(BackpressureInner {
                paused: AtomicBool::new(false),
                notify: Notify::new(),
            }),
        }
    }

    /// Signal that the backend is not writable -- pause client reads.
    #[inline]
    pub fn pause(&self) {
        self.inner.paused.store(true, Ordering::Release);
    }

    /// Signal that the backend has drained -- resume client reads.
    #[inline]
    pub fn resume(&self) {
        self.inner.paused.store(false, Ordering::Release);
        self.inner.notify.notify_waiters();
    }

    /// Returns `true` if client reads should be paused.
    #[inline]
    pub fn is_paused(&self) -> bool {
        self.inner.paused.load(Ordering::Acquire)
    }

    /// Wait until backpressure is released. Returns immediately if not paused.
    ///
    /// This is the correct way to honor backpressure in an async context --
    /// the task sleeps until `resume()` is called, with no busy-spin.
    pub async fn wait_if_paused(&self) {
        while self.is_paused() {
            self.inner.notify.notified().await;
        }
    }
}

impl Default for BackpressureState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_state_is_not_paused() {
        let state = BackpressureState::new();
        assert!(!state.is_paused());
    }

    #[test]
    fn pause_and_resume() {
        let state = BackpressureState::new();
        state.pause();
        assert!(state.is_paused());
        state.resume();
        assert!(!state.is_paused());
    }

    #[test]
    fn clones_share_state() {
        let state1 = BackpressureState::new();
        let state2 = state1.clone();
        state1.pause();
        assert!(state2.is_paused());
        state2.resume();
        assert!(!state1.is_paused());
    }

    #[tokio::test]
    async fn wait_if_paused_returns_immediately_when_not_paused() {
        let state = BackpressureState::new();
        // Should return immediately since not paused.
        state.wait_if_paused().await;
    }

    #[tokio::test]
    async fn wait_if_paused_unblocks_on_resume() {
        let state = BackpressureState::new();
        state.pause();

        let state2 = state.clone();
        let handle = tokio::spawn(async move {
            state2.wait_if_paused().await;
        });

        // Give the spawned task time to register the notified future.
        tokio::task::yield_now().await;

        state.resume();
        // The spawned task should complete now.
        handle.await.unwrap();
    }
}
