//! Write-buffer backpressure control.
//!
//! Tracks pending write bytes and signals when the connection should be
//! paused (high water mark exceeded) or resumed (drained below low water mark).
//! This prevents unbounded memory growth when the downstream is slow.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Default high water mark: 64 KB.
const DEFAULT_HIGH_WATER_MARK: usize = 65_536;

/// Default low water mark: 32 KB.
const DEFAULT_LOW_WATER_MARK: usize = 32_768;

/// Tracks pending write-buffer bytes and enforces high/low water marks
/// to provide flow-control backpressure.
///
/// The controller is safe to share across threads (`Send + Sync`) and uses
/// atomic operations internally.  All operations are lock-free.
#[derive(Debug)]
pub struct BackpressureController {
    /// Current number of pending bytes in the write buffer.
    write_buffer_size: AtomicUsize,
    /// When pending bytes exceed this threshold, the producer should pause.
    high_water_mark: usize,
    /// When pending bytes fall below this threshold, the producer may resume.
    low_water_mark: usize,
    /// Whether the producer is currently paused.
    paused: AtomicBool,
}

impl BackpressureController {
    /// Create a controller with the default water marks (high = 64 KB, low = 32 KB).
    pub fn new() -> Self {
        Self::with_marks(DEFAULT_HIGH_WATER_MARK, DEFAULT_LOW_WATER_MARK)
    }

    /// Create a controller with explicit high and low water marks.
    ///
    /// # Panics
    ///
    /// Panics if `low` > `high`.
    pub fn with_marks(high: usize, low: usize) -> Self {
        assert!(
            low <= high,
            "low water mark ({low}) must be <= high water mark ({high})"
        );
        Self {
            write_buffer_size: AtomicUsize::new(0),
            high_water_mark: high,
            low_water_mark: low,
            paused: AtomicBool::new(false),
        }
    }

    /// Record `bytes` as newly pending in the write buffer.
    ///
    /// Returns `true` if the caller should **pause** producing -- i.e. the
    /// pending bytes now exceed the high water mark.  At most one caller will
    /// receive `true` per pause/resume cycle (CAS on the `paused` flag).
    #[inline]
    pub fn add_pending(&self, bytes: usize) -> bool {
        let prev = self.write_buffer_size.fetch_add(bytes, Ordering::AcqRel);
        let new_size = prev + bytes;
        if new_size >= self.high_water_mark {
            // CAS: only one thread wins the false -> true transition.
            self.paused
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        } else {
            false
        }
    }

    /// Record `bytes` as drained (written to the network).
    ///
    /// Returns `true` if the caller should **resume** producing -- i.e. the
    /// pending bytes dropped below the low water mark while paused.
    ///
    /// Uses a CAS loop to prevent underflow of the atomic counter.
    #[inline]
    pub fn drain(&self, bytes: usize) -> bool {
        // CAS loop to saturate at zero and avoid wrapping.
        let new_size = loop {
            let current = self.write_buffer_size.load(Ordering::Acquire);
            let target = current.saturating_sub(bytes);
            if self
                .write_buffer_size
                .compare_exchange_weak(current, target, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                break target;
            }
        };

        if new_size <= self.low_water_mark {
            // CAS: only one thread wins the true -> false transition.
            self.paused
                .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        } else {
            false
        }
    }

    /// Whether the producer is currently paused due to backpressure.
    #[inline]
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    /// The current number of pending bytes in the write buffer.
    #[inline]
    pub fn pending_bytes(&self) -> usize {
        self.write_buffer_size.load(Ordering::Acquire)
    }

    /// The configured high water mark.
    #[inline]
    pub fn high_water_mark(&self) -> usize {
        self.high_water_mark
    }

    /// The configured low water mark.
    #[inline]
    pub fn low_water_mark(&self) -> usize {
        self.low_water_mark
    }
}

impl Default for BackpressureController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_water_marks() {
        let ctrl = BackpressureController::new();
        assert_eq!(ctrl.high_water_mark(), 65_536);
        assert_eq!(ctrl.low_water_mark(), 32_768);
        assert_eq!(ctrl.pending_bytes(), 0);
        assert!(!ctrl.is_paused());
    }

    #[test]
    fn custom_water_marks() {
        let ctrl = BackpressureController::with_marks(1000, 500);
        assert_eq!(ctrl.high_water_mark(), 1000);
        assert_eq!(ctrl.low_water_mark(), 500);
    }

    #[test]
    #[should_panic(expected = "low water mark")]
    fn panics_on_invalid_marks() {
        BackpressureController::with_marks(100, 200);
    }

    #[test]
    fn pause_on_high_water_mark() {
        let ctrl = BackpressureController::with_marks(100, 50);

        // Add 80 bytes -- below high water mark.
        let should_pause = ctrl.add_pending(80);
        assert!(!should_pause);
        assert!(!ctrl.is_paused());
        assert_eq!(ctrl.pending_bytes(), 80);

        // Add 30 more -- now at 110, exceeds high water mark.
        let should_pause = ctrl.add_pending(30);
        assert!(should_pause);
        assert!(ctrl.is_paused());
        assert_eq!(ctrl.pending_bytes(), 110);
    }

    #[test]
    fn resume_on_low_water_mark() {
        let ctrl = BackpressureController::with_marks(100, 50);

        // Fill past high water mark.
        ctrl.add_pending(120);
        assert!(ctrl.is_paused());

        // Drain 30 bytes -- still at 90, above low water mark.
        let should_resume = ctrl.drain(30);
        assert!(!should_resume);
        assert!(ctrl.is_paused());
        assert_eq!(ctrl.pending_bytes(), 90);

        // Drain 50 more -- now at 40, below low water mark.
        let should_resume = ctrl.drain(50);
        assert!(should_resume);
        assert!(!ctrl.is_paused());
        assert_eq!(ctrl.pending_bytes(), 40);
    }

    #[test]
    fn no_double_pause() {
        let ctrl = BackpressureController::with_marks(100, 50);

        // First add that crosses the threshold signals pause.
        let first = ctrl.add_pending(110);
        assert!(first);

        // Second add while already paused does NOT signal pause again.
        let second = ctrl.add_pending(10);
        assert!(!second);
        assert!(ctrl.is_paused());
    }

    #[test]
    fn no_double_resume() {
        let ctrl = BackpressureController::with_marks(100, 50);

        ctrl.add_pending(150);
        assert!(ctrl.is_paused());

        // First drain below low water mark signals resume.
        let first = ctrl.drain(120);
        assert!(first);
        assert!(!ctrl.is_paused());

        // Second drain while not paused does NOT signal resume.
        let second = ctrl.drain(10);
        assert!(!second);
        assert!(!ctrl.is_paused());
    }

    #[test]
    fn exact_boundary_values() {
        let ctrl = BackpressureController::with_marks(100, 50);

        // Exactly at high water mark triggers pause.
        let should_pause = ctrl.add_pending(100);
        assert!(should_pause);
        assert!(ctrl.is_paused());

        // Drain to exactly the low water mark triggers resume.
        let should_resume = ctrl.drain(50);
        assert!(should_resume);
        assert!(!ctrl.is_paused());
    }

    #[test]
    fn zero_marks() {
        let ctrl = BackpressureController::with_marks(0, 0);
        // Even adding 1 byte exceeds the high water mark.
        let should_pause = ctrl.add_pending(1);
        assert!(should_pause);
    }

    #[test]
    fn drain_does_not_underflow() {
        let ctrl = BackpressureController::with_marks(100, 50);
        ctrl.add_pending(10);
        // Draining more than pending saturates at zero.
        ctrl.drain(20);
        assert_eq!(ctrl.pending_bytes(), 0);
    }

    #[test]
    fn drain_much_more_than_pending_stays_zero() {
        let ctrl = BackpressureController::with_marks(100, 50);
        ctrl.add_pending(5);
        ctrl.drain(1_000_000);
        assert_eq!(ctrl.pending_bytes(), 0);
    }
}
