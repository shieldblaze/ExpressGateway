//! WebSocket backpressure management.
//!
//! Pauses client reads when the backend is not writable and resumes when the
//! backend drains.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Tracks backpressure state between client and backend WebSocket connections.
#[derive(Debug, Clone)]
pub struct BackpressureState {
    /// When `true`, the client side should pause reading frames.
    paused: Arc<AtomicBool>,
}

impl BackpressureState {
    pub fn new() -> Self {
        Self {
            paused: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Signal that the backend is not writable -- pause client reads.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::Release);
    }

    /// Signal that the backend has drained -- resume client reads.
    pub fn resume(&self) {
        self.paused.store(false, Ordering::Release);
    }

    /// Returns `true` if client reads should be paused.
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
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
}
