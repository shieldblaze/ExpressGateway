//! Common type aliases and constants.

use std::net::SocketAddr;

/// Four-tuple connection identifier for L4 proxying.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct FourTuple {
    pub src_addr: SocketAddr,
    pub dst_addr: SocketAddr,
}

/// Backend address for load balancing decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct BackendAddr {
    pub addr: SocketAddr,
}

/// Write buffer water marks for backpressure.
#[derive(Debug, Clone, Copy)]
pub struct WaterMarks {
    /// Pause reads when write buffer exceeds this (bytes).
    pub high: usize,
    /// Resume reads when write buffer drains below this (bytes).
    pub low: usize,
}

impl Default for WaterMarks {
    fn default() -> Self {
        Self {
            high: 65_536, // 64 KB
            low: 32_768,  // 32 KB
        }
    }
}

/// XDP action codes matching kernel definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum XdpAction {
    Drop = 1,
    Pass = 2,
    Tx = 3,
    Redirect = 4,
}

/// Transport protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TransportProtocol {
    Tcp,
    Udp,
}

/// Proxy protocol version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ProxyProtocolVersion {
    V1,
    V2,
}

/// Mutual TLS mode matching Java implementation.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default,
)]
pub enum MutualTlsMode {
    /// No client certificate requested.
    #[default]
    NotRequired,
    /// Client certificate requested but connection allowed without.
    Optional,
    /// Client certificate mandatory.
    Required,
}

/// TLS profile controlling cipher suite selection.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default,
)]
pub enum TlsProfile {
    /// TLS 1.3 only, 3 ciphers.
    Modern,
    /// TLS 1.2 + 1.3, 7 ciphers.
    #[default]
    Intermediate,
}

/// Direction for metrics tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Upstream,
    Downstream,
}

/// Environment mode.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default,
)]
pub enum Environment {
    #[default]
    Production,
    Development,
}
