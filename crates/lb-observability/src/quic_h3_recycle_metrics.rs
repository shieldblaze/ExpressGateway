//! `h3_*` connection-recycling metric family for the H3-front actor
//! (S36-A — `max_requests_per_h3_connection` cap → GOAWAY → drain →
//! recycle).
//!
//! Mirrors the [`QuicModeBMetrics`](crate::quic_modeb_metrics::QuicModeBMetrics)
//! subsystem pattern: a `Clone` struct of `prometheus` handles built once
//! via [`QuicH3RecycleMetrics::register`] off the shared
//! [`MetricsRegistry`](crate::MetricsRegistry) get-or-create API. The
//! H3-termination router (in `lb-quic`) holds an
//! `Option<QuicH3RecycleMetrics>` (always `Some` once a non-Mode-B QUIC
//! listener is spawned with a metrics registry) and bumps the handles from
//! the per-connection actor at the two recycle event sites — GOAWAY-sent
//! and connection-recycled. The handles are `Arc`-backed inside
//! `prometheus`, so cloning them into the per-connection actors is cheap.
//!
//! This is the H3 sibling of the H1/H2 `keepalive_cap_terminations`
//! counter (`lb_l7::h1_proxy`): both observe the gateway proactively
//! closing a connection that hit its per-connection request cap.

use prometheus::IntCounter;

use crate::{MetricsError, MetricsRegistry};

/// `h3_*` connection-recycling family handles. Cheap to clone.
#[derive(Clone, Debug)]
pub struct QuicH3RecycleMetrics {
    /// `h3_goaway_sent_total` — cumulative count of H3 GOAWAY frames the
    /// gateway sent because a connection reached
    /// `max_requests_per_h3_connection`. One increment per connection that
    /// hits the cap (GOAWAY is sent at most once per connection). Distinct
    /// from a GOAWAY sent on drain/shutdown for other reasons (none of
    /// which exists today — the only GOAWAY emitter is the cap path).
    pub goaway_sent_total: IntCounter,
    /// `h3_connections_recycled_total` — cumulative count of H3
    /// connections the gateway gracefully closed after a cap-triggered
    /// GOAWAY drained every in-flight request/tunnel. One increment per
    /// connection that completed the cap → GOAWAY → drain → close
    /// lifecycle. May lag `goaway_sent_total` by the connections that were
    /// sent a GOAWAY but then idle-timed-out (or the client closed first)
    /// before the explicit drain-close ran.
    pub connections_recycled_total: IntCounter,
}

impl QuicH3RecycleMetrics {
    /// Register every `h3_*` recycle family against `registry`.
    ///
    /// Idempotent: the registry's get-or-create semantics return the
    /// existing handle if a name was already registered, so a second QUIC
    /// listener spawn reuses the same metrics. All handles read 0 from
    /// registration so `/metrics` shows the rows from spawn (the soak +
    /// the recycle-count e2e assert against them).
    ///
    /// # Errors
    ///
    /// Bubbles up the underlying `prometheus` registration error (or a
    /// [`MetricsError::TypeMismatch`] if a name was already registered
    /// under a different metric type).
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
        // A second register against the same registry must return the
        // SAME handles, observing the increment (not a fresh zero).
        let b = QuicH3RecycleMetrics::register(&reg).expect("second");
        assert_eq!(b.goaway_sent_total.get(), 1);
    }
}
