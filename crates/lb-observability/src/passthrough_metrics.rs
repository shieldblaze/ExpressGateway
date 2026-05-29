//! `quic_passthrough_*` metric family for the Mode A QUIC passthrough
//! datapath (S15 A3 — `audit/quic/s15-design.md` §A3).
//!
//! Mirrors the [`XdpMetrics`](crate::xdp_metrics::XdpMetrics) subsystem
//! pattern: a `Clone` struct of `prometheus` handles built once via
//! [`PassthroughMetrics::register`] off the shared
//! [`MetricsRegistry`](crate::MetricsRegistry) get-or-create API. The
//! passthrough router (in `lb-quic`) holds an `Option<PassthroughMetrics>`
//! and bumps the handles at its event sites; the handles are
//! `Arc`-backed inside `prometheus` so cloning them into the per-flow
//! tasks is cheap.

use prometheus::{IntCounter, IntGauge};

use crate::{MetricsError, MetricsRegistry};

/// `quic_passthrough_*` family handles. Cheap to clone.
#[derive(Clone, Debug)]
pub struct PassthroughMetrics {
    /// `quic_passthrough_flows` — current dispatch-table size. Set to
    /// `table.len()` after each insert/eviction; a migrated flow may
    /// briefly hold 2 CID keys.
    pub flows: IntGauge,
    /// `quic_passthrough_flows_evicted_total` — LRU evictions
    /// (one increment per evicted flow, not per removed CID key).
    pub flows_evicted_total: IntCounter,
    /// `quic_passthrough_retry_minted_total` — stateless Retry packets
    /// minted and sent back to clients.
    pub retry_minted_total: IntCounter,
    /// `quic_passthrough_retry_rejected_total` — Retry-token verify
    /// failures (the Initial is dropped).
    pub retry_rejected_total: IntCounter,
    /// `quic_passthrough_header_parse_errors_total` — public-header
    /// parse failures on inbound datagrams.
    pub header_parse_errors_total: IntCounter,
    /// `quic_passthrough_backend_socket_errors_total` — per-flow backend
    /// UDP socket errors (bind/connect/send/recv).
    pub backend_socket_errors_total: IntCounter,
}

impl PassthroughMetrics {
    /// Register every `quic_passthrough_*` family against `registry`.
    ///
    /// Idempotent: the registry's get-or-create semantics return the
    /// existing handle if a name was already registered, so a second
    /// listener spawn reuses the same metrics. All handles read 0 from
    /// registration (gauge starts 0, counters start 0) so `/metrics`
    /// shows the rows from spawn.
    ///
    /// # Errors
    ///
    /// Bubbles up the underlying `prometheus` registration error (or a
    /// [`MetricsError::TypeMismatch`] if a name was already registered
    /// under a different metric type).
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
        // A second register against the same registry must return the
        // SAME handles, observing the increment (not a fresh zero).
        let b = PassthroughMetrics::register(&reg).expect("second");
        assert_eq!(b.retry_minted_total.get(), 1);
    }
}
