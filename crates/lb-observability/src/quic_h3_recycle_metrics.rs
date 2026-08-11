//! `h3_*` metrics for the connection-recycle path: cap → GOAWAY → drain → close. The H3 sibling
//! of the H1/H2 `keepalive_cap_terminations` counter.

use prometheus::IntCounter;

use crate::{MetricsError, MetricsRegistry};

/// `h3_*` connection-recycling family handles. Cheap to clone.
#[derive(Clone, Debug)]
pub struct QuicH3RecycleMetrics {
    /// GOAWAYs sent on hitting `max_requests_per_h3_connection`; at most one per connection.
    pub goaway_sent_total: IntCounter,
    /// Connections that completed the full cap → GOAWAY → drain → close cycle. LAGS
    /// `goaway_sent_total` by those that idle-timed-out or were closed by the client first.
    pub connections_recycled_total: IntCounter,
}

impl QuicH3RecycleMetrics {
    /// Register every family. Idempotent; all handles read 0 from registration so the soak and
    /// recycle-count e2e can assert against the rows immediately.
    pub fn register(registry: &MetricsRegistry) -> Result<Self, MetricsError> {
        let goaway_sent_total = registry.counter(
            "h3_goaway_sent_total",
            "Cumulative H3 GOAWAY frames sent because a connection reached max_requests_per_h3_connection (the recycle cap).",
        )?;
        let connections_recycled_total = registry.counter(
            "h3_connections_recycled_total",
            "Cumulative H3 connections gracefully closed after a cap-triggered GOAWAY drained all in-flight requests (recycled).",
        )?;
        Ok(Self {
            goaway_sent_total,
            connections_recycled_total,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_seeds_all_families_at_zero() {
        let reg = MetricsRegistry::new();
        let m = QuicH3RecycleMetrics::register(&reg).expect("register");
        assert_eq!(m.goaway_sent_total.get(), 0);
        assert_eq!(m.connections_recycled_total.get(), 0);
    }

    #[test]
    fn register_is_idempotent() {
        let reg = MetricsRegistry::new();
        let a = QuicH3RecycleMetrics::register(&reg).expect("first");
        a.goaway_sent_total.inc();
        // A second register must return the SAME handles, not fresh zeros.
        let b = QuicH3RecycleMetrics::register(&reg).expect("second");
        assert_eq!(b.goaway_sent_total.get(), 1);
    }
}
