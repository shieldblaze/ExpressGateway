//! Health state tracking with sliding sample window.

use std::collections::VecDeque;

use expressgateway_core::Health;

/// Tracks health check results per node with a sliding sample window.
///
/// Uses rise/fall thresholds to determine when a node should transition
/// between online and offline states.
pub struct HealthTracker {
    results: VecDeque<bool>,
    samples: u32,
    consecutive_success: u32,
    consecutive_failure: u32,
    rise: u32,
    fall: u32,
}

impl HealthTracker {
    /// Create a new health tracker with the given parameters.
    pub fn new(samples: u32, rise: u32, fall: u32) -> Self {
        Self {
            results: VecDeque::with_capacity(samples as usize),
            samples,
            consecutive_success: 0,
            consecutive_failure: 0,
            rise,
            fall,
        }
    }

    /// Record a health check result.
    pub fn record(&mut self, success: bool) {
        // Evict oldest if at capacity.
        if self.results.len() >= self.samples as usize {
            self.results.pop_front();
        }
        self.results.push_back(success);

        if success {
            self.consecutive_success += 1;
            self.consecutive_failure = 0;
        } else {
            self.consecutive_failure += 1;
            self.consecutive_success = 0;
        }
    }

    /// Current health assessment based on success rate.
    pub fn health(&self) -> Health {
        if self.results.is_empty() {
            return Health::Unknown;
        }

        let total = self.results.len() as f64;
        let successes = self.results.iter().filter(|&&s| s).count() as f64;
        let rate = successes / total;

        if rate >= 0.95 {
            Health::Good
        } else if rate >= 0.75 {
            Health::Medium
        } else {
            Health::Bad
        }
    }

    /// Whether the node should transition to ONLINE (consecutive successes >= rise).
    pub fn should_go_online(&self) -> bool {
        self.consecutive_success >= self.rise
    }

    /// Whether the node should transition to OFFLINE (consecutive failures >= fall).
    pub fn should_go_offline(&self) -> bool {
        self.consecutive_failure >= self.fall
    }

    /// Number of recorded samples.
    pub fn sample_count(&self) -> usize {
        self.results.len()
    }

    /// Current consecutive success count.
    pub fn consecutive_successes(&self) -> u32 {
        self.consecutive_success
    }

    /// Current consecutive failure count.
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failure
    }
}

impl Default for HealthTracker {
    fn default() -> Self {
        Self::new(100, 2, 3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_tracker_returns_unknown() {
        let tracker = HealthTracker::default();
        assert_eq!(tracker.health(), Health::Unknown);
        assert!(!tracker.should_go_online());
        assert!(!tracker.should_go_offline());
    }

    #[test]
    fn test_all_successes_returns_good() {
        let mut tracker = HealthTracker::new(10, 2, 3);
        for _ in 0..10 {
            tracker.record(true);
        }
        assert_eq!(tracker.health(), Health::Good);
    }

    #[test]
    fn test_all_failures_returns_bad() {
        let mut tracker = HealthTracker::new(10, 2, 3);
        for _ in 0..10 {
            tracker.record(false);
        }
        assert_eq!(tracker.health(), Health::Bad);
    }

    #[test]
    fn test_medium_health() {
        let mut tracker = HealthTracker::new(20, 2, 3);
        // 18 successes, 2 failures = 90% -> Medium
        for _ in 0..18 {
            tracker.record(true);
        }
        for _ in 0..2 {
            tracker.record(false);
        }
        assert_eq!(tracker.health(), Health::Medium);
    }

    #[test]
    fn test_boundary_95_percent() {
        let mut tracker = HealthTracker::new(20, 2, 3);
        // 19 successes, 1 failure = 95% -> Good
        for _ in 0..19 {
            tracker.record(true);
        }
        tracker.record(false);
        assert_eq!(tracker.health(), Health::Good);
    }

    #[test]
    fn test_boundary_75_percent() {
        let mut tracker = HealthTracker::new(20, 2, 3);
        // 15 successes, 5 failures = 75% -> Medium
        for _ in 0..15 {
            tracker.record(true);
        }
        for _ in 0..5 {
            tracker.record(false);
        }
        assert_eq!(tracker.health(), Health::Medium);
    }

    #[test]
    fn test_below_75_percent() {
        let mut tracker = HealthTracker::new(20, 2, 3);
        // 14 successes, 6 failures = 70% -> Bad
        for _ in 0..14 {
            tracker.record(true);
        }
        for _ in 0..6 {
            tracker.record(false);
        }
        assert_eq!(tracker.health(), Health::Bad);
    }

    #[test]
    fn test_rise_threshold() {
        let mut tracker = HealthTracker::new(10, 3, 3);
        tracker.record(true);
        assert!(!tracker.should_go_online());
        tracker.record(true);
        assert!(!tracker.should_go_online());
        tracker.record(true);
        assert!(tracker.should_go_online());
    }

    #[test]
    fn test_fall_threshold() {
        let mut tracker = HealthTracker::new(10, 2, 3);
        tracker.record(false);
        assert!(!tracker.should_go_offline());
        tracker.record(false);
        assert!(!tracker.should_go_offline());
        tracker.record(false);
        assert!(tracker.should_go_offline());
    }

    #[test]
    fn test_consecutive_reset_on_alternation() {
        let mut tracker = HealthTracker::new(10, 2, 3);
        tracker.record(true);
        tracker.record(true);
        assert!(tracker.should_go_online());

        // A failure resets consecutive success
        tracker.record(false);
        assert_eq!(tracker.consecutive_successes(), 0);
        assert_eq!(tracker.consecutive_failures(), 1);
        assert!(!tracker.should_go_online());
    }

    #[test]
    fn test_sliding_window_eviction() {
        let mut tracker = HealthTracker::new(5, 2, 3);
        // Fill with successes
        for _ in 0..5 {
            tracker.record(true);
        }
        assert_eq!(tracker.sample_count(), 5);
        assert_eq!(tracker.health(), Health::Good);

        // Add failures; old successes get evicted
        for _ in 0..5 {
            tracker.record(false);
        }
        assert_eq!(tracker.sample_count(), 5);
        assert_eq!(tracker.health(), Health::Bad);
    }
}
