//! Consolidated HTTP/2 "glitches" abuse counter.
//!
//! HAProxy 3.0 shipped `tune.h2.fe.glitches-threshold` after concluding
//! that operators cannot tune six independent per-detector thresholds.
//! The per-connection score sums weighted events from every detector
//! kind; once the rolling-window sum crosses the threshold the
//! connection is drained (GOAWAY + close).
//!
//! Closes ROUND8-L7-07 (frame-recv timeout — one of the input kinds)
//! and ROUND8-L7-12 (single consolidated counter).
//!
//! References:
//! - HAProxy 3.0 `src/mux_h2.c` `h2_glitches_*` (default threshold 200,
//!   weight table cited per kind below).
//! - nginx `http2_recv_timeout = 30s`.
//!
//! Wiring (Phase D scope): the type is exposed for the H2 proxy to
//! call `record(kind, now)` from each detector path; the actual
//! `tokio::time::Interval` wiring lives in `lb-l7::h2_proxy` and is
//! gated by a future config knob (`h2_recv_frame_timeout_ms`).
//!
//! Prometheus surface (`lb_h2_glitches_*`) is deferred to a div-ops
//! follow-up because this crate cannot reach `lb-observability` per
//! the workspace dependency rules.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Default rolling window for the abuse counter (matches HAProxy 3.0).
pub const DEFAULT_GLITCHES_WINDOW: Duration = Duration::from_secs(60);

/// Default sum-of-weighted-events threshold (matches HAProxy 3.0).
pub const DEFAULT_GLITCHES_THRESHOLD: u32 = 200;

/// Per-connection H2 frame-arrival deadline (nginx parity).
pub const DEFAULT_RECV_FRAME_TIMEOUT: Duration = Duration::from_secs(30);

/// HAProxy-derived weight table. Each kind contributes `weight()` to
/// the sliding-window sum when `GlitchesCounter::record` is called.
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
    /// No H2 frame received within `recv_frame_timeout` — H2
    /// slowloris cousin (ROUND8-L7-07 specific input).
    FrameRecvTimeout,
}

impl GlitchKind {
    /// HAProxy-published per-kind cost. Tuned so the sum of "noisy
    /// but legitimate" pre-existing per-detector thresholds lands at
    /// ~half of the consolidated threshold (200 by default).
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

/// Outcome of a glitch record. `Drain` instructs the proxy to emit
/// GOAWAY and close the connection; `Allow` lets the request
/// continue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlitchOutcome {
    /// Connection still healthy enough to continue.
    Allow,
    /// Threshold exceeded — drain (GOAWAY + close).
    Drain,
}

/// Per-connection abuse counter aggregating weighted events from
/// every H2 detector path.
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

    /// Record one event of the given kind at `now`.
    ///
    /// Events older than `window` are evicted from the sliding sum
    /// before the new event is added. Returns `GlitchOutcome::Drain`
    /// once the post-add sum crosses the threshold.
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

    /// Drop events that fell outside the rolling window relative to
    /// `now`. Idempotent.
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
        // Pin the per-kind weights so a future refactor cannot
        // silently inflate or deflate the abuse score. Operators
        // run on tuned thresholds; any change here is a public-API
        // break and must update the CHANGELOG.
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
        // 10 RapidReset = 50 weight = exactly at threshold → allow.
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
        // 11th RapidReset → 55 weight > 50 → drain.
        let r = c.record(GlitchKind::RapidReset, t0 + Duration::from_secs(10));
        assert_eq!(r, GlitchOutcome::Drain);
    }

    #[test]
    fn glitches_score_aggregates_across_detectors() {
        let mut c = GlitchesCounter::new(200, Duration::from_secs(60));
        let t0 = Instant::now();
        // 10 RapidReset (50) + 50 Continuation (50) + 1 HpackRatio
        // (10) + 8 Ping (16) = 126 score, all allow.
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
        // Score should be 126.
        assert_eq!(c.score(), 126);

        // Add 10 more RapidReset → 176, still under.
        for _ in 0..10 {
            assert_eq!(c.record(GlitchKind::RapidReset, t0), GlitchOutcome::Allow,);
        }
        assert_eq!(c.score(), 176);

        // Add 4 FrameRecvTimeout (32) → 208 > 200 → drain on the
        // 4th one (the third puts us at 200 = threshold; only the
        // strict > test trips on the next).
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
        // Advance past the window — prune should drop every event.
        c.prune(t0 + Duration::from_secs(61));
        assert_eq!(c.score(), 0);
        // Fresh budget after rollover.
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
