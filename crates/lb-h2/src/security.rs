//! HTTP/2 security mitigation detectors.
//!
//! - **Rapid Reset** (CVE-2023-44487): detects excessive `RST_STREAM` frames.
//! - **CONTINUATION Flood** (CVE-2024-24549): detects long runs of
//!   CONTINUATION frames without `END_HEADERS`.
//! - **HPACK Bomb**: detects decompression-ratio amplification attacks.

use crate::H2Error;

/// Detects rapid-reset attacks by counting `RST_STREAM` frames using a
/// sliding-window approximation (two-bucket algorithm).
///
/// A single fixed-window counter can be bypassed by concentrating events
/// at a window boundary (~2x the threshold across two adjacent windows).
/// This implementation keeps a previous-window count and weights it by the
/// fraction of overlap, identical to the technique used by nginx rate
/// limiting.
///
/// Uses O(1) memory and integer-only arithmetic (no floating point).
///
/// The window is measured in arbitrary "ticks" provided by the caller.
/// In production, pass `Instant::now()` converted to a monotonic counter.
/// For testing, pass synthetic tick values.
#[derive(Debug)]
pub struct RapidResetDetector {
    threshold: u64,
    window_ticks: u64,
    window_start: u64,
    count_in_window: u64,
    prev_count: u64,
}

impl RapidResetDetector {
    /// Create a detector.
    ///
    /// * `threshold` — maximum allowed `RST_STREAM` frames per window.
    /// * `window_ticks` — window duration in caller-defined tick units.
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
    ///
    /// Returns `H2Error::RapidReset` if the sliding-window estimated count
    /// exceeds the threshold.
    pub fn record(&mut self, tick: u64) -> Result<(), H2Error> {
        let elapsed = tick.saturating_sub(self.window_start);

        if elapsed > self.window_ticks {
            // Current tick is outside the window — rotate.
            // If the tick jumped more than two windows, prev_count is zero
            // because the previous window had no events we can attribute.
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

        // Sliding-window estimate using integer math (scaled by 1000).
        //
        // The standard two-bucket formula (nginx, Cloudflare, etc.):
        //   estimated = prev_count * (1 - elapsed_fraction) + count_in_window
        //
        // `count_in_window` is taken at full weight because those events
        // definitely occurred within the current observation interval.
        // `prev_count` is scaled down by how far we are into the current
        // window — the further in, the less the previous window overlaps.
        let elapsed_in_current = tick.saturating_sub(self.window_start);
        let elapsed_fraction_x1000 = if self.window_ticks > 0 {
            (elapsed_in_current.saturating_mul(1000)) / self.window_ticks
        } else {
            1000
        };
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

/// Detects CONTINUATION flood attacks by counting CONTINUATION frames
/// received without an `END_HEADERS` flag.
#[derive(Debug)]
pub struct ContinuationFloodDetector {
    max_continuations: u64,
    count: u64,
}

impl ContinuationFloodDetector {
    /// Create a detector with the given maximum allowed CONTINUATION frames
    /// per header block.
    #[must_use]
    pub const fn new(max_continuations: u64) -> Self {
        Self {
            max_continuations,
            count: 0,
        }
    }

    /// Record a CONTINUATION frame.
    ///
    /// * `end_headers` — if `true`, the counter resets (header block complete).
    ///
    /// # Errors
    ///
    /// Returns `H2Error::ContinuationFlood` if the limit is exceeded.
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

/// Detects HPACK decompression bombs by tracking the ratio of decoded header
/// size to encoded wire size.
#[derive(Debug)]
pub struct HpackBombDetector {
    max_ratio: u64,
    max_decoded_size: u64,
}

impl HpackBombDetector {
    /// Create a detector.
    ///
    /// * `max_ratio` — maximum allowed decoded/encoded byte ratio.
    /// * `max_decoded_size` — absolute cap on decoded header bytes.
    #[must_use]
    pub const fn new(max_ratio: u64, max_decoded_size: u64) -> Self {
        Self {
            max_ratio,
            max_decoded_size,
        }
    }

    /// Check whether the given encoded and decoded sizes are within limits.
    ///
    /// # Errors
    ///
    /// Returns `H2Error::HpackBomb` if either the ratio or absolute size
    /// exceeds the configured limits.
    pub const fn check(&self, encoded_size: u64, decoded_size: u64) -> Result<(), H2Error> {
        if decoded_size > self.max_decoded_size {
            let ratio = if encoded_size > 0 {
                decoded_size / encoded_size
            } else {
                decoded_size
            };
            return Err(H2Error::HpackBomb {
                decoded: decoded_size,
                encoded: encoded_size,
                ratio,
            });
        }

        if encoded_size > 0 {
            let ratio = decoded_size / encoded_size;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rapid_reset_under_threshold() {
        let mut det = RapidResetDetector::new(5, 100);
        // All events at tick 0 — count_in_window grows but prev_count is 0.
        // At tick 0, elapsed_in_current = 0, so weight_current = 0, weight_prev = 1000.
        // estimated = prev_count * 1000 + count * 0 = 0.
        // That means the very first window only uses prev_count for the estimate
        // at tick 0 itself. We need to spread ticks across the window.
        for i in 1..=5 {
            assert!(det.record(i * 20).is_ok());
        }
    }

    #[test]
    fn rapid_reset_over_threshold() {
        let mut det = RapidResetDetector::new(5, 100);
        // Spread events across the window so the sliding estimate reflects them.
        for i in 1..=5 {
            assert!(det.record(i * 18).is_ok());
        }
        // 6th event at tick 99 — still within the window, estimate should exceed 5.
        assert!(det.record(99).is_err());
    }

    #[test]
    fn rapid_reset_window_expiry() {
        let mut det = RapidResetDetector::new(5, 100);
        for i in 0..5 {
            assert!(det.record(i).is_ok());
        }
        // Jump two full windows ahead — prev_count is zeroed.
        assert!(det.record(300).is_ok());
    }

    #[test]
    fn rapid_reset_boundary_attack_detected() {
        // Boundary attack: with a fixed-window counter, an attacker sends
        // `threshold` events at the end of one window and another burst at
        // the start of the next, achieving 2x the allowed rate because the
        // counter resets to 0.
        //
        // The sliding window carries over prev_count, so the first event in
        // the new window pushes the weighted estimate above threshold.
        let threshold = 10u64;
        let window = 100u64;
        let mut det = RapidResetDetector::new(threshold, window);

        // Fill the first window with exactly `threshold` events evenly spaced.
        // In the first window prev_count = 0, so the estimate equals
        // count_in_window, which reaches exactly threshold (not exceeded).
        for i in 1..=threshold {
            assert!(
                det.record(i * 10).is_ok(),
                "event {i} at tick {} should be allowed in the first window",
                i * 10,
            );
        }

        // Window rotates at tick 101. prev_count becomes 10.
        // estimated = prev_count * weight_prev + count_in_window * 1000
        //           = 10 * 1000 + 1 * 1000  (at elapsed_in_current = 0)
        //           = 11000 > threshold * 1000 = 10000
        //
        // The very first event after rotation is detected because the
        // previous window was at capacity.
        assert!(
            det.record(101).is_err(),
            "first event after a full window should be detected by the \
             sliding-window carry-over",
        );
    }

    #[test]
    fn rapid_reset_sliding_window_decays() {
        // Verify that the previous window's influence decays as we progress
        // through the current window. Early in the window the carry-over is
        // strong; later it fades and allows more room.
        let threshold = 10u64;
        let window = 100u64;
        let mut det = RapidResetDetector::new(threshold, window);

        // Fill the first window with 5 events.
        for i in 1..=5 {
            det.record(i * 20).ok();
        }

        // Rotate into the second window with a single event at tick 101.
        // prev_count = 5. estimated = 5*1000 + 1*1000 = 6000 <= 10000. OK.
        assert!(det.record(101).is_ok());

        // Now skip to tick 180 — 79 ticks into the second window.
        // elapsed_in_current = 79, weight_prev = 1000 - 790 = 210.
        // count_in_window = 2. estimated = 5 * 210 + 2 * 1000 = 1050 + 2000 = 3050. OK.
        assert!(det.record(180).is_ok());

        // Continue bursting from tick 181. The low prev weight allows more events.
        // tick 181: count=3, elapsed=80, wp=200. est = 5*200 + 3*1000 = 4000. OK.
        // tick 182: count=4, elapsed=81, wp=190. est = 5*190 + 4*1000 = 4950. OK.
        // tick 183: count=5, elapsed=82, wp=180. est = 5*180 + 5*1000 = 5900. OK.
        // tick 184: count=6, elapsed=83, wp=170. est = 5*170 + 6*1000 = 6850. OK.
        // tick 185: count=7, elapsed=84, wp=160. est = 5*160 + 7*1000 = 7800. OK.
        // tick 186: count=8, elapsed=85, wp=150. est = 5*150 + 8*1000 = 8750. OK.
        // tick 187: count=9, elapsed=86, wp=140. est = 5*140 + 9*1000 = 9700. OK.
        // tick 188: count=10,elapsed=87, wp=130. est = 5*130 + 10*1000 = 10650. ERR.
        for tick in 181..=187 {
            assert!(det.record(tick).is_ok(), "tick {tick} should be allowed");
        }
        assert!(
            det.record(188).is_err(),
            "at tick 188 the estimate exceeds threshold",
        );

        // Compare: at the start of the window (tick 101-106), we could only
        // fit 5 events before detection. Late in the window (tick 180-187)
        // we fit 8 events — the decay allowed more headroom.
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
}
