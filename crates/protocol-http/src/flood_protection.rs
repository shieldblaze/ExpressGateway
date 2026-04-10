//! HTTP/2 control frame flood protection.
//!
//! Rate-limits SETTINGS and PING frames to a maximum of 100 per 10-second
//! sliding window. When the limit is exceeded, the connection should be
//! terminated with a GOAWAY frame using `ENHANCE_YOUR_CALM` (error code 0xb).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// HTTP/2 `ENHANCE_YOUR_CALM` error code (0x000000b).
pub const ENHANCE_YOUR_CALM: u32 = 0xb;

/// Default maximum control frames per window.
pub const DEFAULT_MAX_CONTROL_FRAMES: usize = 100;

/// Default sliding window duration.
pub const DEFAULT_WINDOW_DURATION: Duration = Duration::from_secs(10);

/// Control frame types tracked for flood protection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlFrameType {
    /// HTTP/2 SETTINGS frame.
    Settings,
    /// HTTP/2 PING frame.
    Ping,
}

/// Sliding-window rate limiter for HTTP/2 control frames.
///
/// Tracks timestamps of recent SETTINGS and PING frames and rejects new
/// frames when the count exceeds the configured maximum within the window.
pub struct ControlFrameRateLimiter {
    /// Maximum number of control frames allowed per window.
    max_frames: usize,
    /// Duration of the sliding window.
    window: Duration,
    /// Timestamps of control frames within the current window.
    timestamps: VecDeque<Instant>,
}

impl ControlFrameRateLimiter {
    /// Create a new rate limiter with default settings (100 frames per 10s).
    pub fn new() -> Self {
        Self::with_config(DEFAULT_MAX_CONTROL_FRAMES, DEFAULT_WINDOW_DURATION)
    }

    /// Create a new rate limiter with custom settings.
    pub fn with_config(max_frames: usize, window: Duration) -> Self {
        Self {
            max_frames,
            window,
            timestamps: VecDeque::with_capacity(max_frames + 1),
        }
    }

    /// Record a control frame and check if the rate limit is exceeded.
    ///
    /// Returns `true` if the frame is allowed, `false` if it should trigger
    /// a GOAWAY with `ENHANCE_YOUR_CALM`.
    pub fn check_frame(&mut self, _frame_type: ControlFrameType) -> bool {
        self.check_frame_at(_frame_type, Instant::now())
    }

    /// Record a control frame at a specific time (for testing).
    fn check_frame_at(&mut self, _frame_type: ControlFrameType, now: Instant) -> bool {
        // Evict timestamps outside the window.
        let cutoff = now.checked_sub(self.window).unwrap_or(now);
        while let Some(&front) = self.timestamps.front() {
            if front < cutoff {
                self.timestamps.pop_front();
            } else {
                break;
            }
        }

        // Check rate.
        if self.timestamps.len() >= self.max_frames {
            return false;
        }

        self.timestamps.push_back(now);
        true
    }

    /// Number of control frames in the current window.
    pub fn current_count(&self) -> usize {
        self.timestamps.len()
    }

    /// Reset the rate limiter state.
    pub fn reset(&mut self) {
        self.timestamps.clear();
    }
}

impl Default for ControlFrameRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_frames_under_limit() {
        let mut limiter = ControlFrameRateLimiter::with_config(5, Duration::from_secs(10));
        for _ in 0..5 {
            assert!(limiter.check_frame(ControlFrameType::Settings));
        }
    }

    #[test]
    fn rejects_frames_over_limit() {
        let mut limiter = ControlFrameRateLimiter::with_config(3, Duration::from_secs(10));
        assert!(limiter.check_frame(ControlFrameType::Settings));
        assert!(limiter.check_frame(ControlFrameType::Ping));
        assert!(limiter.check_frame(ControlFrameType::Settings));
        // 4th frame should be rejected.
        assert!(!limiter.check_frame(ControlFrameType::Ping));
    }

    #[test]
    fn window_expiry_allows_new_frames() {
        let mut limiter = ControlFrameRateLimiter::with_config(2, Duration::from_secs(1));
        let base = Instant::now();

        // Fill up the window.
        assert!(limiter.check_frame_at(ControlFrameType::Settings, base));
        assert!(limiter.check_frame_at(ControlFrameType::Ping, base));
        assert!(!limiter.check_frame_at(ControlFrameType::Settings, base));

        // After the window expires, frames should be allowed again.
        let later = base + Duration::from_secs(2);
        assert!(limiter.check_frame_at(ControlFrameType::Settings, later));
    }

    #[test]
    fn current_count_tracks_frames() {
        let mut limiter = ControlFrameRateLimiter::with_config(10, Duration::from_secs(10));
        assert_eq!(limiter.current_count(), 0);
        limiter.check_frame(ControlFrameType::Settings);
        assert_eq!(limiter.current_count(), 1);
        limiter.check_frame(ControlFrameType::Ping);
        assert_eq!(limiter.current_count(), 2);
    }

    #[test]
    fn reset_clears_state() {
        let mut limiter = ControlFrameRateLimiter::with_config(10, Duration::from_secs(10));
        limiter.check_frame(ControlFrameType::Settings);
        limiter.check_frame(ControlFrameType::Ping);
        assert_eq!(limiter.current_count(), 2);
        limiter.reset();
        assert_eq!(limiter.current_count(), 0);
    }

    #[test]
    fn default_config() {
        let limiter = ControlFrameRateLimiter::new();
        assert_eq!(limiter.max_frames, 100);
        assert_eq!(limiter.window, Duration::from_secs(10));
    }

    #[test]
    fn enhance_your_calm_constant() {
        assert_eq!(ENHANCE_YOUR_CALM, 0xb);
    }

    #[test]
    fn mixed_frame_types_share_budget() {
        let mut limiter = ControlFrameRateLimiter::with_config(4, Duration::from_secs(10));
        assert!(limiter.check_frame(ControlFrameType::Settings));
        assert!(limiter.check_frame(ControlFrameType::Ping));
        assert!(limiter.check_frame(ControlFrameType::Settings));
        assert!(limiter.check_frame(ControlFrameType::Ping));
        // 5th frame -- either type -- should be rejected.
        assert!(!limiter.check_frame(ControlFrameType::Settings));
        assert!(!limiter.check_frame(ControlFrameType::Ping));
    }
}
