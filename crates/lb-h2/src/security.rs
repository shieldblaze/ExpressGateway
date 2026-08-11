//! HTTP/2 security mitigation detectors: Rapid Reset (CVE-2023-44487),
//! CONTINUATION flood (CVE-2024-24549), HPACK bomb, SETTINGS/PING flood, and
//! the zero-window stall watchdog.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::H2Error;

/// Detects rapid-reset attacks over a two-bucket sliding window (the nginx
/// technique), O(1) memory and integer-only.
///
/// THE CATCH: a single FIXED-window counter is bypassable by concentrating
/// events at a window boundary — ~2x the threshold across two adjacent
/// windows. Keeping the previous-window count, weighted by overlap, closes it.
#[derive(Debug)]
pub struct RapidResetDetector {
    threshold: u64,
    window_ticks: u64,
    window_start: u64,
    count_in_window: u64,
    prev_count: u64,
}

impl RapidResetDetector {
    /// Create a detector; `window_ticks` is in caller-defined tick units.
    #[must_use]
    pub const fn new(threshold: u64, window_ticks: u64) -> Self {
        Self {
            threshold,
            window_ticks,
            window_start: 0,
            count_in_window: 0,
            prev_count: 0,
        }
    }

    /// Record a `RST_STREAM` event at the given tick.
    ///
    /// # Errors
    /// `H2Error::RapidReset` once the sliding-window estimate exceeds the
    /// threshold.
    pub fn record(&mut self, tick: u64) -> Result<(), H2Error> {
        let elapsed = tick.saturating_sub(self.window_start);

        if elapsed > self.window_ticks {
            // A jump of more than two windows leaves prev_count zero.
            if elapsed > self.window_ticks.saturating_mul(2) {
                self.prev_count = 0;
            } else {
                self.prev_count = self.count_in_window;
            }
            self.window_start = tick;
            self.count_in_window = 1;
        } else {
            self.count_in_window += 1;
        }

        // Two-bucket estimate, integer math scaled by 1000:
        //   estimated = prev_count * (1 - elapsed_fraction) + count_in_window
        // `count_in_window` is FULL weight; `prev_count` decays.
        let elapsed_in_current = tick.saturating_sub(self.window_start);
        let elapsed_fraction_x1000 = elapsed_in_current
            .saturating_mul(1000)
            .checked_div(self.window_ticks)
            .unwrap_or(1000);
        let weight_prev_x1000 = 1000u64.saturating_sub(elapsed_fraction_x1000);
        let estimated_x1000 = self
            .prev_count
            .saturating_mul(weight_prev_x1000)
            .saturating_add(self.count_in_window.saturating_mul(1000));
        let threshold_x1000 = self.threshold.saturating_mul(1000);

        if estimated_x1000 > threshold_x1000 {
            Err(H2Error::RapidReset {
                count: self.count_in_window,
            })
        } else {
            Ok(())
        }
    }

    /// Reset the detector state.
    pub fn reset(&mut self) {
        self.window_start = 0;
        self.count_in_window = 0;
        self.prev_count = 0;
    }
}

/// Counts CONTINUATION frames received without an `END_HEADERS` flag.
#[derive(Debug)]
pub struct ContinuationFloodDetector {
    max_continuations: u64,
    count: u64,
}

impl ContinuationFloodDetector {
    /// Create a detector capped at `max` CONTINUATION frames per header block.
    #[must_use]
    pub const fn new(max_continuations: u64) -> Self {
        Self {
            max_continuations,
            count: 0,
        }
    }

    /// Record a CONTINUATION frame; `end_headers` resets the counter.
    ///
    /// # Errors
    /// `H2Error::ContinuationFlood` once the limit is exceeded.
    pub fn record(&mut self, end_headers: bool) -> Result<(), H2Error> {
        if end_headers {
            self.count = 0;
            return Ok(());
        }

        self.count += 1;
        if self.count > self.max_continuations {
            Err(H2Error::ContinuationFlood { count: self.count })
        } else {
            Ok(())
        }
    }

    /// Reset the detector state.
    pub fn reset(&mut self) {
        self.count = 0;
    }
}

/// Tracks the decoded/encoded header-size ratio for HPACK bombs.
#[derive(Debug)]
pub struct HpackBombDetector {
    max_ratio: u64,
    max_decoded_size: u64,
}

impl HpackBombDetector {
    /// Create a detector with a ratio limit and an absolute decoded-size cap.
    #[must_use]
    pub const fn new(max_ratio: u64, max_decoded_size: u64) -> Self {
        Self {
            max_ratio,
            max_decoded_size,
        }
    }

    /// Check encoded/decoded sizes against the limits.
    ///
    /// # Errors
    /// `H2Error::HpackBomb` if either the ratio or the absolute size trips.
    pub const fn check(&self, encoded_size: u64, decoded_size: u64) -> Result<(), H2Error> {
        if decoded_size > self.max_decoded_size {
            let ratio = match decoded_size.checked_div(encoded_size) {
                Some(r) => r,
                None => decoded_size,
            };
            return Err(H2Error::HpackBomb {
                decoded: decoded_size,
                encoded: encoded_size,
                ratio,
            });
        }

        if let Some(ratio) = decoded_size.checked_div(encoded_size) {
            if ratio > self.max_ratio {
                return Err(H2Error::HpackBomb {
                    decoded: decoded_size,
                    encoded: encoded_size,
                    ratio,
                });
            }
        }

        Ok(())
    }
}

/// Default SETTINGS frames per window for `SettingsFloodDetector`.
pub const DEFAULT_SETTINGS_MAX_PER_WINDOW: u32 = 100;

/// Default PING frames per window for `PingFloodDetector`.
pub const DEFAULT_PING_MAX_PER_WINDOW: u32 = 50;

/// Default rolling-window duration for SETTINGS and PING flood detectors.
pub const DEFAULT_CONTROL_FRAME_WINDOW: Duration = Duration::from_secs(10);

/// Default stall timeout for `ZeroWindowStallDetector`.
pub const DEFAULT_ZERO_WINDOW_STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Detects `SETTINGS` flooding over a rolling window: each `SETTINGS` obliges
/// the peer to allocate and transmit an ACK, so the attacker's send cost
/// amplifies into defender CPU and bandwidth (as Pingora and nginx both note).
#[derive(Debug)]
pub struct SettingsFloodDetector {
    max_per_window: u32,
    window: Duration,
    window_start: Option<Instant>,
    count_in_window: u32,
}

impl SettingsFloodDetector {
    /// Create a detector with explicit thresholds.
    #[must_use]
    pub const fn new(max_per_window: u32, window: Duration) -> Self {
        Self {
            max_per_window,
            window,
            window_start: None,
            count_in_window: 0,
        }
    }

    /// Create a detector with the project-default thresholds.
    #[must_use]
    pub const fn with_defaults() -> Self {
        Self::new(
            DEFAULT_SETTINGS_MAX_PER_WINDOW,
            DEFAULT_CONTROL_FRAME_WINDOW,
        )
    }

    /// Record a `SETTINGS` frame observation at `now`.
    ///
    /// # Errors
    /// `H2Error::SettingsFlood` past `max_per_window`.
    pub fn on_settings(&mut self, now: Instant) -> Result<(), H2Error> {
        let rotate = match self.window_start {
            None => true,
            Some(start) => now.saturating_duration_since(start) >= self.window,
        };
        if rotate {
            self.window_start = Some(now);
            self.count_in_window = 1;
        } else {
            self.count_in_window = self.count_in_window.saturating_add(1);
        }
        if self.count_in_window > self.max_per_window {
            Err(H2Error::SettingsFlood {
                count: self.count_in_window,
            })
        } else {
            Ok(())
        }
    }

    /// Reset the detector state.
    pub fn reset(&mut self) {
        self.window_start = None;
        self.count_in_window = 0;
    }
}

/// `PING` flood detector; same rolling window as [`SettingsFloodDetector`].
/// Every `PING` obliges an ACK, forcing reads AND writes at attacker rates.
#[derive(Debug)]
pub struct PingFloodDetector {
    max_per_window: u32,
    window: Duration,
    window_start: Option<Instant>,
    count_in_window: u32,
}

impl PingFloodDetector {
    /// Create a detector with explicit thresholds.
    #[must_use]
    pub const fn new(max_per_window: u32, window: Duration) -> Self {
        Self {
            max_per_window,
            window,
            window_start: None,
            count_in_window: 0,
        }
    }

    /// Create a detector with the project-default thresholds.
    #[must_use]
    pub const fn with_defaults() -> Self {
        Self::new(DEFAULT_PING_MAX_PER_WINDOW, DEFAULT_CONTROL_FRAME_WINDOW)
    }

    /// Record a `PING` frame observation at `now`.
    ///
    /// # Errors
    /// `H2Error::PingFlood` past `max_per_window`.
    pub fn on_ping(&mut self, now: Instant) -> Result<(), H2Error> {
        let rotate = match self.window_start {
            None => true,
            Some(start) => now.saturating_duration_since(start) >= self.window,
        };
        if rotate {
            self.window_start = Some(now);
            self.count_in_window = 1;
        } else {
            self.count_in_window = self.count_in_window.saturating_add(1);
        }
        if self.count_in_window > self.max_per_window {
            Err(H2Error::PingFlood {
                count: self.count_in_window,
            })
        } else {
            Ok(())
        }
    }

    /// Reset the detector state.
    pub fn reset(&mut self) {
        self.window_start = None;
        self.count_in_window = 0;
    }
}

/// Detects streams starved of receive-window credit. THE CATCH: every
/// `WINDOW_UPDATE` refreshes the entry, but only a NON-ZERO `increment` counts
/// as progress and advances `last_progress`.
#[derive(Debug)]
pub struct ZeroWindowStallDetector {
    stall_timeout: Duration,
    last_progress: HashMap<u32, Instant>,
}

impl ZeroWindowStallDetector {
    /// Create a detector with an explicit stall timeout.
    #[must_use]
    pub fn new(stall_timeout: Duration) -> Self {
        Self {
            stall_timeout,
            last_progress: HashMap::new(),
        }
    }

    /// Create a detector with the project-default stall timeout (30s).
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_ZERO_WINDOW_STALL_TIMEOUT)
    }

    /// Record a `WINDOW_UPDATE`; only a NON-ZERO `increment` counts as
    /// progress and restarts the stall watchdog.
    pub fn on_window_update(&mut self, stream_id: u32, increment: u32, now: Instant) {
        let entry = self.last_progress.entry(stream_id).or_insert(now);
        if increment > 0 {
            *entry = now;
        }
    }

    /// Has this stream been stalled longer than `stall_timeout`?
    #[must_use]
    pub fn check_stalled(&self, stream_id: u32, now: Instant) -> bool {
        self.last_progress
            .get(&stream_id)
            .is_some_and(|last| now.saturating_duration_since(*last) > self.stall_timeout)
    }

    /// Forget a stream, after close.
    pub fn remove_stream(&mut self, stream_id: u32) {
        self.last_progress.remove(&stream_id);
    }

    /// Forget every stream.
    pub fn reset(&mut self) {
        self.last_progress.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rapid_reset_under_threshold() {
        let mut det = RapidResetDetector::new(5, 100);
        // Ticks must be SPREAD: the estimate weights `prev_count` by how far
        // into the current window we are.
        for i in 1..=5 {
            assert!(det.record(i * 20).is_ok());
        }
    }

    #[test]
    fn rapid_reset_over_threshold() {
        let mut det = RapidResetDetector::new(5, 100);
        for i in 1..=5 {
            assert!(det.record(i * 18).is_ok());
        }
        assert!(det.record(99).is_err());
    }

    #[test]
    fn rapid_reset_window_expiry() {
        let mut det = RapidResetDetector::new(5, 100);
        for i in 0..5 {
            assert!(det.record(i).is_ok());
        }
        assert!(det.record(300).is_ok());
    }

    #[test]
    fn rapid_reset_boundary_attack_detected() {
        // Boundary attack: a FIXED-window counter lets an attacker straddle two
        // windows for 2x the rate. Carrying prev_count over closes it.
        let threshold = 10u64;
        let window = 100u64;
        let mut det = RapidResetDetector::new(threshold, window);

        for i in 1..=threshold {
            assert!(
                det.record(i * 10).is_ok(),
                "event {i} at tick {} should be allowed in the first window",
                i * 10,
            );
        }

        // prev_count = 10 after rotation, so the FIRST event past it detects.
        assert!(
            det.record(101).is_err(),
            "first event after a full window should be detected by the \
             sliding-window carry-over",
        );
    }

    #[test]
    fn rapid_reset_sliding_window_decays() {
        // The previous window's influence must DECAY across the current one.
        let threshold = 10u64;
        let window = 100u64;
        let mut det = RapidResetDetector::new(threshold, window);

        for i in 1..=5 {
            det.record(i * 20).ok();
        }

        assert!(det.record(101).is_ok());

        assert!(det.record(180).is_ok());

        for tick in 181..=187 {
            assert!(det.record(tick).is_ok(), "tick {tick} should be allowed");
        }
        assert!(
            det.record(188).is_err(),
            "at tick 188 the estimate exceeds threshold",
        );

        // 5 events fit early in the window, 8 late — that gap IS the decay.
    }

    #[test]
    fn continuation_flood_ok() {
        let mut det = ContinuationFloodDetector::new(5);
        for _ in 0..5 {
            assert!(det.record(false).is_ok());
        }
        assert!(det.record(true).is_ok());
    }

    #[test]
    fn continuation_flood_exceeded() {
        let mut det = ContinuationFloodDetector::new(5);
        for _ in 0..5 {
            assert!(det.record(false).is_ok());
        }
        assert!(det.record(false).is_err());
    }

    #[test]
    fn hpack_bomb_ok() {
        let det = HpackBombDetector::new(100, 65536);
        assert!(det.check(1000, 2000).is_ok());
    }

    #[test]
    fn hpack_bomb_ratio_exceeded() {
        let det = HpackBombDetector::new(100, 1_000_000);
        assert!(det.check(1024, 204_800).is_err());
    }

    #[test]
    fn hpack_bomb_size_exceeded() {
        let det = HpackBombDetector::new(100, 65536);
        assert!(det.check(10_000, 100_000).is_err());
    }

    #[test]
    fn settings_under_limit_allowed() {
        let mut det = SettingsFloodDetector::new(5, Duration::from_secs(10));
        let t0 = Instant::now();
        for i in 0..5 {
            assert!(det.on_settings(t0 + Duration::from_millis(i * 100)).is_ok());
        }
    }

    #[test]
    fn settings_burst_rejected() {
        let mut det = SettingsFloodDetector::new(5, Duration::from_secs(10));
        let t0 = Instant::now();
        for i in 0..5 {
            assert!(det.on_settings(t0 + Duration::from_millis(i * 100)).is_ok());
        }
        let err = det
            .on_settings(t0 + Duration::from_millis(600))
            .unwrap_err();
        assert!(matches!(err, H2Error::SettingsFlood { count: 6 }));
    }

    #[test]
    fn settings_resets_after_window() {
        let mut det = SettingsFloodDetector::new(5, Duration::from_secs(10));
        let t0 = Instant::now();
        for i in 0..5 {
            assert!(det.on_settings(t0 + Duration::from_millis(i * 100)).is_ok());
        }
        assert!(det.on_settings(t0 + Duration::from_secs(11)).is_ok());
        for i in 1..5 {
            assert!(
                det.on_settings(t0 + Duration::from_secs(11) + Duration::from_millis(i * 100))
                    .is_ok()
            );
        }
    }

    #[test]
    fn ping_under_limit_allowed() {
        let mut det = PingFloodDetector::new(3, Duration::from_secs(10));
        let t0 = Instant::now();
        for i in 0..3 {
            assert!(det.on_ping(t0 + Duration::from_millis(i * 50)).is_ok());
        }
    }

    #[test]
    fn ping_burst_rejected() {
        let mut det = PingFloodDetector::new(3, Duration::from_secs(10));
        let t0 = Instant::now();
        for i in 0..3 {
            assert!(det.on_ping(t0 + Duration::from_millis(i * 50)).is_ok());
        }
        let err = det.on_ping(t0 + Duration::from_millis(200)).unwrap_err();
        assert!(matches!(err, H2Error::PingFlood { count: 4 }));
    }

    #[test]
    fn zero_window_stall_fires_after_timeout() {
        let mut det = ZeroWindowStallDetector::new(Duration::from_secs(5));
        let t0 = Instant::now();
        // A ZERO increment seeds the entry but is not progress.
        det.on_window_update(1, 0, t0);
        assert!(!det.check_stalled(1, t0 + Duration::from_secs(4)));
        assert!(det.check_stalled(1, t0 + Duration::from_secs(6)));
    }

    #[test]
    fn zero_window_stall_reset_on_progress() {
        let mut det = ZeroWindowStallDetector::new(Duration::from_secs(5));
        let t0 = Instant::now();
        det.on_window_update(7, 0, t0);
        // A positive increment advances last_progress.
        det.on_window_update(7, 1024, t0 + Duration::from_secs(4));
        assert!(!det.check_stalled(7, t0 + Duration::from_secs(6)));
        assert!(det.check_stalled(7, t0 + Duration::from_secs(12)));
        det.remove_stream(7);
        assert!(!det.check_stalled(7, t0 + Duration::from_secs(99)));
    }
}
