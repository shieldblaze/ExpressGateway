//! `quic_modeb_*` metric family for the Mode B (terminate-and-
//! re-originate) raw-QUIC proxy datapath (S19 B6 —
//! `audit/quic/s19-b5-b6-plan.md` §B6.3).
//!
//! Mirrors the [`PassthroughMetrics`](crate::passthrough_metrics::PassthroughMetrics)
//! subsystem pattern: a `Clone` struct of `prometheus` handles built once
//! via [`QuicModeBMetrics::register`] off the shared
//! [`MetricsRegistry`](crate::MetricsRegistry) get-or-create API. The
//! Mode-B router (in `lb-quic`) holds an `Option<QuicModeBMetrics>` and
//! bumps the handles at its actor-lifetime / per-pass event sites; the
//! handles are `Arc`-backed inside `prometheus` so cloning them into the
//! per-connection actors is cheap.
//!
//! The handles are bumped only at the relay actor lifetime + once-per-pass
//! aggregate sites (NOT inside the per-stream / per-datagram helpers), so
//! the B4/B5 relay logic is untouched — see `lb_quic::raw_proxy`.

use prometheus::{IntCounter, IntGauge};

use crate::{MetricsError, MetricsRegistry};

/// `quic_modeb_*` family handles. Cheap to clone.
#[derive(Clone, Debug)]
pub struct QuicModeBMetrics {
    /// `quic_modeb_connections` — currently-active Mode-B proxied
    /// connections. Incremented when an actor's upstream leg is
    /// established (the two-conn relay is live) and decremented when the
    /// actor returns (graceful close, idle timeout, or cancel).
    pub connections: IntGauge,
    /// `quic_modeb_connections_total` — cumulative count of established
    /// two-connection relays (one increment per actor that reaches the
    /// re-originated-upstream-established state).
    pub connections_total: IntCounter,
    /// `quic_modeb_datagrams_dropped_total` — cumulative DATAGRAM
    /// drop-newest events across all Mode-B relays, surfaced from the B4
    /// `BoundedDgramQueue::dropped()` accessor (summed over both queues,
    /// updated by delta after each relay pass).
    pub datagrams_dropped_total: IntCounter,
    /// `quic_modeb_streams_active` — current relay-stream table size
    /// (the B5 bound), set to the table length after each relay pass.
    /// A connection-level aggregate gauge: with one actor it tracks that
    /// actor's table; under many concurrent actors it reflects the most
    /// recent set (sufficient for the bounded-state visibility intent).
    pub streams_active: IntGauge,
}

impl QuicModeBMetrics {
    /// Register every `quic_modeb_*` family against `registry`.
    ///
    /// Idempotent: the registry's get-or-create semantics return the
    /// existing handle if a name was already registered, so a second
    /// Mode-B listener spawn reuses the same metrics. All handles read 0
    /// from registration (gauges start 0, counters start 0) so `/metrics`
    /// shows the rows from spawn.
    ///
    /// # Errors
    ///
    /// Bubbles up the underlying `prometheus` registration error (or a
    /// [`MetricsError::TypeMismatch`] if a name was already registered
    /// under a different metric type).
    pub fn register(registry: &MetricsRegistry) -> Result<Self, MetricsError> {
        let connections = registry.gauge(
            "quic_modeb_connections",
            "Active Mode-B (terminate-and-re-originate) raw-QUIC proxied connections (two-conn relays currently live).",
        )?;
        let connections_total = registry.counter(
            "quic_modeb_connections_total",
            "Cumulative established Mode-B two-connection raw-QUIC relays.",
        )?;
        let datagrams_dropped_total = registry.counter(
            "quic_modeb_datagrams_dropped_total",
            "Cumulative QUIC DATAGRAM (RFC 9221) drop-newest events in the Mode-B relay (bounded-queue overflow).",
        )?;
        let streams_active = registry.gauge(
            "quic_modeb_streams_active",
            "Current Mode-B relay-stream table size (the B5 per-connection bounded-state ceiling).",
        )?;
        Ok(Self {
            connections,
            connections_total,
            datagrams_dropped_total,
            streams_active,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_seeds_all_families_at_zero() {
        let reg = MetricsRegistry::new();
        let m = QuicModeBMetrics::register(&reg).expect("register");
        assert_eq!(m.connections.get(), 0);
        assert_eq!(m.connections_total.get(), 0);
        assert_eq!(m.datagrams_dropped_total.get(), 0);
        assert_eq!(m.streams_active.get(), 0);
    }

    #[test]
    fn register_is_idempotent() {
        let reg = MetricsRegistry::new();
        let a = QuicModeBMetrics::register(&reg).expect("first");
        a.connections_total.inc();
        // A second register against the same registry must return the
        // SAME handles, observing the increment (not a fresh zero).
        let b = QuicModeBMetrics::register(&reg).expect("second");
        assert_eq!(b.connections_total.get(), 1);
    }
}
