//! Backend types: upstream server representation, health status, and runtime state.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// Represents an upstream backend server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Backend {
    id: String,
    address: SocketAddr,
    weight: u32,
}

impl Backend {
    /// Create a new backend with the given id, address, and weight.
    #[must_use]
    pub const fn new(id: String, address: SocketAddr, weight: u32) -> Self {
        Self {
            id,
            address,
            weight,
        }
    }

    /// Returns the backend identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the socket address of the backend.
    #[must_use]
    pub const fn address(&self) -> SocketAddr {
        self.address
    }

    /// Returns the backend weight for weighted algorithms.
    #[must_use]
    pub const fn weight(&self) -> u32 {
        self.weight
    }
}

/// Health status of a backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendHealth {
    /// Backend is healthy and accepting traffic.
    Healthy,
    /// Backend is unhealthy and should not receive traffic.
    Unhealthy,
    /// Health status is not yet determined.
    Unknown,
}

impl Default for BackendHealth {
    fn default() -> Self {
        Self::Unknown
    }
}

/// Runtime state tracking for a backend: connections, requests, and latency.
///
/// Uses atomics for lock-free concurrent access on the hot path.
#[derive(Debug)]
pub struct BackendState {
    active_connections: AtomicU64,
    active_requests: AtomicU64,
    latency_ns: AtomicU64,
}

impl BackendState {
    /// Create a new zeroed backend state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            active_connections: AtomicU64::new(0),
            active_requests: AtomicU64::new(0),
            latency_ns: AtomicU64::new(0),
        }
    }

    /// Returns the current number of active connections.
    #[must_use]
    pub fn active_connections(&self) -> u64 {
        self.active_connections.load(Ordering::Relaxed)
    }

    /// Increment the active connection count by one.
    pub fn inc_connections(&self) {
        self.active_connections.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement the active connection count by one (saturating).
    pub fn dec_connections(&self) {
        // Saturating: fetch_sub with underflow protection via compare-exchange loop.
        let mut current = self.active_connections.load(Ordering::Relaxed);
        loop {
            let new = current.saturating_sub(1);
            match self.active_connections.compare_exchange_weak(
                current,
                new,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    /// Returns the current number of active requests.
    #[must_use]
    pub fn active_requests(&self) -> u64 {
        self.active_requests.load(Ordering::Relaxed)
    }

    /// Increment the active request count by one.
    pub fn inc_requests(&self) {
        self.active_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement the active request count by one (saturating).
    pub fn dec_requests(&self) {
        let mut current = self.active_requests.load(Ordering::Relaxed);
        loop {
            let new = current.saturating_sub(1);
            match self.active_requests.compare_exchange_weak(
                current,
                new,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    /// Returns the last recorded response latency in nanoseconds.
    #[must_use]
    pub fn latency_ns(&self) -> u64 {
        self.latency_ns.load(Ordering::Relaxed)
    }

    /// Sets the response latency in nanoseconds.
    pub fn set_latency_ns(&self, ns: u64) {
        self.latency_ns.store(ns, Ordering::Relaxed);
    }
}

impl Default for BackendState {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for BackendState {
    fn clone(&self) -> Self {
        Self {
            active_connections: AtomicU64::new(self.active_connections.load(Ordering::Relaxed)),
            active_requests: AtomicU64::new(self.active_requests.load(Ordering::Relaxed)),
            latency_ns: AtomicU64::new(self.latency_ns.load(Ordering::Relaxed)),
        }
    }
}
