//! `quic_passthrough_*` metrics for the Mode A datapath. Handles are `Arc`-backed, so cloning
//! them into per-flow tasks is cheap.

use prometheus::{IntCounter, IntGauge};

use crate::{MetricsError, MetricsRegistry};

/// `quic_passthrough_*` family handles. Cheap to clone.
#[derive(Clone, Debug)]
pub struct PassthroughMetrics {
    /// Current dispatch-table size. A migrated flow briefly holds 2 CID keys.
    pub flows: IntGauge,
    /// LRU evictions — one per FLOW, not per removed CID key.
    pub flows_evicted_total: IntCounter,
    /// Stateless Retry packets minted.
    pub retry_minted_total: IntCounter,
    /// Retry-token verify failures; the Initial is dropped.
    pub retry_rejected_total: IntCounter,
    /// Public-header parse failures on inbound datagrams.
    pub header_parse_errors_total: IntCounter,
    /// Per-flow backend UDP socket errors.
    pub backend_socket_errors_total: IntCounter,
}

impl PassthroughMetrics {
    /// Register every family. Idempotent, and all handles read 0 so `/metrics` shows the rows at once.
    pub fn register(registry: &MetricsRegistry) -> Result<Self, MetricsError> {
        let flows = registry.gauge(
            "quic_passthrough_flows",
            "Active QUIC passthrough dispatch-table entries (~ flows; a migrated flow may briefly hold 2 CID keys).",
        )?;
        let flows_evicted_total = registry.counter(
            "quic_passthrough_flows_evicted_total",
            "QUIC passthrough flows evicted from the dispatch table under the LRU cap.",
        )?;
        let retry_minted_total = registry.counter(
            "quic_passthrough_retry_minted_total",
            "Stateless Retry packets minted by the QUIC passthrough listener.",
        )?;
        let retry_rejected_total = registry.counter(
            "quic_passthrough_retry_rejected_total",
            "Initials dropped by the QUIC passthrough listener on Retry-token verify failure.",
        )?;
        let header_parse_errors_total = registry.counter(
            "quic_passthrough_header_parse_errors_total",
            "Inbound datagrams dropped by the QUIC passthrough listener on public-header parse failure.",
        )?;
        let backend_socket_errors_total = registry.counter(
            "quic_passthrough_backend_socket_errors_total",
            "Per-flow backend UDP socket errors (bind/connect/send/recv) in the QUIC passthrough datapath.",
        )?;
        Ok(Self {
            flows,
            flows_evicted_total,
            retry_minted_total,
            retry_rejected_total,
            header_parse_errors_total,
            backend_socket_errors_total,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_seeds_all_families_at_zero() {
        let reg = MetricsRegistry::new();
        let m = PassthroughMetrics::register(&reg).expect("register");
        assert_eq!(m.flows.get(), 0);
        assert_eq!(m.flows_evicted_total.get(), 0);
        assert_eq!(m.retry_minted_total.get(), 0);
        assert_eq!(m.retry_rejected_total.get(), 0);
        assert_eq!(m.header_parse_errors_total.get(), 0);
        assert_eq!(m.backend_socket_errors_total.get(), 0);
    }

    #[test]
    fn register_is_idempotent() {
        let reg = MetricsRegistry::new();
        let a = PassthroughMetrics::register(&reg).expect("first");
        a.retry_minted_total.inc();
        // A second register must return the SAME handles, not fresh zeros.
        let b = PassthroughMetrics::register(&reg).expect("second");
        assert_eq!(b.retry_minted_total.get(), 1);
    }
}
