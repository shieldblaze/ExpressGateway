//! Consolidated HTTP/2 "glitches" abuse counter (HAProxy 3.0 `tune.h2.fe.glitches-threshold`): one
//! weighted rolling-window score across all H2 detectors, because operators cannot tune six
//! independent thresholds. Crossing it drains the connection (GOAWAY + close).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Default rolling window for the abuse counter (matches HAProxy 3.0).
pub const DEFAULT_GLITCHES_WINDOW: Duration = Duration::from_secs(60);

/// Default sum-of-weighted-events threshold (matches HAProxy 3.0).
pub const DEFAULT_GLITCHES_THRESHOLD: u32 = 200;

/// Per-connection H2 frame-arrival deadline (nginx parity).
pub const DEFAULT_RECV_FRAME_TIMEOUT: Duration = Duration::from_secs(30);

/// Event kinds feeding the score; see [`GlitchKind::weight`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlitchKind {
    /// `RST_STREAM` burst (CVE-2023-44487 rapid-reset class).
    RapidReset,
    /// CONTINUATION-frame chain without END_HEADERS (CVE-2024-27316).
    ContinuationFlood,
    /// SETTINGS-frame flood (each forces an ACK).
    SettingsFlood,
    /// PING-frame flood (each forces an ACK).
    PingFlood,
    /// HPACK decompression-ratio exceeded (bomb attempt).
    HpackRatio,
    /// Stream stalled with zero-window credit advances.
    ZeroWindowStall,
    /// No H2 frame within `recv_frame_timeout` — the H2 slowloris cousin.
    FrameRecvTimeout,
}

impl GlitchKind {
    /// HAProxy-published per-kind cost. Changing any weight is a public-API break — pinned by
    /// `weights_match_haproxy_table`.
    #[must_use]
    pub const fn weight(self) -> u32 {
        match self {
            Self::ContinuationFlood => 1,
            Self::SettingsFlood | Self::PingFlood => 2,
            Self::RapidReset | Self::ZeroWindowStall => 5,
            Self::FrameRecvTimeout => 8,
            Self::HpackRatio => 10,
        }
    }
}

/// Outcome of a [`GlitchesCounter::record`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlitchOutcome {
    /// Connection still healthy enough to continue.
    Allow,
    /// Threshold exceeded — drain (GOAWAY + close).
    Drain,
}

/// Per-connection abuse counter over every H2 detector path.
#[derive(Debug)]
pub struct GlitchesCounter {
    window: Duration,
    threshold: u32,
    events: VecDeque<(Instant, u32)>,
    sum_in_window: u32,
}

impl GlitchesCounter {
    /// Create a counter with explicit window + threshold values.
    #[must_use]
    pub const fn new(threshold: u32, window: Duration) -> Self {
        Self {
            window,
            threshold,
            events: VecDeque::new(),
            sum_in_window: 0,
        }
    }

    /// Create a counter with the HAProxy-3.0 defaults.
    #[must_use]
    pub const fn with_defaults() -> Self {
        Self::new(DEFAULT_GLITCHES_THRESHOLD, DEFAULT_GLITCHES_WINDOW)
    }

    /// Configured threshold (events-weight sum within the window).
    #[must_use]
    pub const fn threshold(&self) -> u32 {
        self.threshold
    }

    /// Configured rolling window.
    #[must_use]
    pub const fn window(&self) -> Duration {
        self.window
    }

    /// Current sum within the window (gauge surface).
    #[must_use]
    pub const fn score(&self) -> u32 {
        self.sum_in_window
    }

    /// Record one event at `now`, pruning first. Drains only on a STRICT `>`; equal still allows.
    pub fn record(&mut self, kind: GlitchKind, now: Instant) -> GlitchOutcome {
        self.prune(now);

        let weight = kind.weight();
        self.events.push_back((now, weight));
        self.sum_in_window = self.sum_in_window.saturating_add(weight);

        if self.sum_in_window > self.threshold {
            GlitchOutcome::Drain
        } else {
            GlitchOutcome::Allow
        }
    }

    /// Drop events outside the window relative to `now`. Idempotent.
    pub fn prune(&mut self, now: Instant) {
        let cutoff = now.checked_sub(self.window);
        if let Some(cutoff) = cutoff {
            while let Some(&(when, weight)) = self.events.front() {
                if when < cutoff {
                    self.events.pop_front();
                    self.sum_in_window = self.sum_in_window.saturating_sub(weight);
                } else {
                    break;
                }
            }
        }
    }
}

impl Default for GlitchesCounter {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_match_haproxy_table() {
        // Any change here is a public-API break for operators running tuned thresholds.
        assert_eq!(GlitchKind::ContinuationFlood.weight(), 1);
        assert_eq!(GlitchKind::SettingsFlood.weight(), 2);
        assert_eq!(GlitchKind::PingFlood.weight(), 2);
        assert_eq!(GlitchKind::RapidReset.weight(), 5);
        assert_eq!(GlitchKind::ZeroWindowStall.weight(), 5);
        assert_eq!(GlitchKind::FrameRecvTimeout.weight(), 8);
        assert_eq!(GlitchKind::HpackRatio.weight(), 10);
    }

    #[test]
    fn under_threshold_allows() {
        let mut c = GlitchesCounter::new(50, Duration::from_secs(60));
        let t0 = Instant::now();
        for i in 0..10 {
            let r = c.record(GlitchKind::RapidReset, t0 + Duration::from_secs(i));
            assert_eq!(r, GlitchOutcome::Allow, "event #{i} should be allowed");
        }
    }

    #[test]
    fn crossing_threshold_drains() {
        let mut c = GlitchesCounter::new(50, Duration::from_secs(60));
        let t0 = Instant::now();
        for i in 0..10 {
            let _ = c.record(GlitchKind::RapidReset, t0 + Duration::from_secs(i));
        }
        let r = c.record(GlitchKind::RapidReset, t0 + Duration::from_secs(10));
        assert_eq!(r, GlitchOutcome::Drain);
    }

    #[test]
    fn glitches_score_aggregates_across_detectors() {
        let mut c = GlitchesCounter::new(200, Duration::from_secs(60));
        let t0 = Instant::now();
        for _ in 0..10 {
            assert_eq!(c.record(GlitchKind::RapidReset, t0), GlitchOutcome::Allow,);
        }
        for _ in 0..50 {
            assert_eq!(
                c.record(GlitchKind::ContinuationFlood, t0),
                GlitchOutcome::Allow,
            );
        }
        assert_eq!(c.record(GlitchKind::HpackRatio, t0), GlitchOutcome::Allow,);
        for _ in 0..8 {
            assert_eq!(c.record(GlitchKind::PingFlood, t0), GlitchOutcome::Allow);
        }
        assert_eq!(c.score(), 126);

        for _ in 0..10 {
            assert_eq!(c.record(GlitchKind::RapidReset, t0), GlitchOutcome::Allow,);
        }
        assert_eq!(c.score(), 176);

        // The 3rd lands exactly on 200 and still allows; only the 4th trips the strict `>`.
        assert_eq!(
            c.record(GlitchKind::FrameRecvTimeout, t0),
            GlitchOutcome::Allow, // 184
        );
        assert_eq!(
            c.record(GlitchKind::FrameRecvTimeout, t0),
            GlitchOutcome::Allow, // 192
        );
        assert_eq!(
            c.record(GlitchKind::FrameRecvTimeout, t0),
            GlitchOutcome::Allow, // 200 (== threshold)
        );
        assert_eq!(
            c.record(GlitchKind::FrameRecvTimeout, t0),
            GlitchOutcome::Drain, // 208 > 200
        );
    }

    #[test]
    fn counter_resets_after_window() {
        let mut c = GlitchesCounter::new(50, Duration::from_secs(60));
        let t0 = Instant::now();
        for _ in 0..10 {
            let _ = c.record(GlitchKind::RapidReset, t0);
        }
        assert_eq!(c.score(), 50);
        c.prune(t0 + Duration::from_secs(61));
        assert_eq!(c.score(), 0);
        assert_eq!(
            c.record(GlitchKind::RapidReset, t0 + Duration::from_secs(61)),
            GlitchOutcome::Allow,
        );
    }

    #[test]
    fn defaults_match_haproxy_3_0() {
        let c = GlitchesCounter::with_defaults();
        assert_eq!(c.threshold(), 200);
        assert_eq!(c.window(), Duration::from_secs(60));
    }
}
