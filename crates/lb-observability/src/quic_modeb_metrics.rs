//! `quic_modeb_*` metrics for the Mode B raw-QUIC proxy.
//!
//! Bumped ONLY at actor-lifetime and once-per-pass sites, never inside the per-stream or
//! per-datagram helpers, so the relay hot path stays untouched.

use prometheus::{IntCounter, IntGauge};

use crate::{MetricsError, MetricsRegistry};

/// `quic_modeb_*` family handles. Cheap to clone.
#[derive(Clone, Debug)]
pub struct QuicModeBMetrics {
    /// Active Mode-B relays: up when the upstream leg establishes, down when the actor returns.
    pub connections: IntGauge,
    /// Cumulative established two-connection relays.
    pub connections_total: IntCounter,
    /// DATAGRAM drop-newest events, summed over both queues and updated by delta per pass.
    pub datagrams_dropped_total: IntCounter,
    /// Relay-stream table size after each pass. Under concurrent actors this reflects only the
    /// MOST RECENT actor's table, not a sum — it is a bounded-state signal, not a total.
    pub streams_active: IntGauge,
}

impl QuicModeBMetrics {
    /// Register every family. Idempotent; all handles read 0 so `/metrics` shows the rows from
    /// spawn.
    ///
    /// # Errors
    ///
    /// The `prometheus` registration error, or [`MetricsError::TypeMismatch`].
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
        // A second register must return the SAME handles, not fresh zeros.
        let b = QuicModeBMetrics::register(&reg).expect("second");
        assert_eq!(b.connections_total.get(), 1);
    }
}
