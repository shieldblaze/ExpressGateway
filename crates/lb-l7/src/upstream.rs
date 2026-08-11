//! Upstream backend dispatch for the L7 proxies (PROTO-001).
//!
//! The proxies historically dialed every backend over [`lb_io::pool::TcpPool`]
//! and spoke HTTP/1.1 on the upstream wire. PROTO-001 branches on a
//! per-backend protocol selector to reach H2 and H3 backends. This module
//! ships the public types expressing it: [`UpstreamProto`],
//! [`UpstreamBackend`], [`BackendInfoPicker`], the [`SingleProtoPicker`]
//! adapter for the H1-only call sites that pre-date PROTO-001, and
//! [`RoundRobinUpstreams`].

use std::net::SocketAddr;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::h1_proxy::BackendPicker;

/// Upstream wire protocol for a backend dial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpstreamProto {
    /// HTTP/1.1 over plain TCP (matches `Backend::protocol = "tcp"` and
    /// `"h1"` — both treated identically by the L7 proxies).
    H1,
    /// HTTP/2 over plain TCP, via [`lb_io::http2_pool::Http2Pool`].
    H2,
    /// HTTP/3 over QUIC, via [`lb_io::quic_pool::QuicUpstreamPool`].
    H3,
}

/// Resolved upstream backend descriptor: peer address, wire protocol, and an
/// optional SNI for H3 (whose TLS handshake demands a server-name authority).
#[derive(Debug, Clone)]
pub struct UpstreamBackend {
    /// Peer socket address.
    pub addr: SocketAddr,
    /// Wire protocol the proxy must speak to this backend.
    pub proto: UpstreamProto,
    /// SNI string. Required for `H3`, ignored for `H1`/`H2` — hence `Option`.
    pub sni: Option<String>,
}

impl UpstreamBackend {
    /// Construct a plain H1 backend with no SNI.
    #[must_use]
    pub const fn h1(addr: SocketAddr) -> Self {
        Self {
            addr,
            proto: UpstreamProto::H1,
            sni: None,
        }
    }

    /// Construct a plain H2 backend with no SNI.
    #[must_use]
    pub const fn h2(addr: SocketAddr) -> Self {
        Self {
            addr,
            proto: UpstreamProto::H2,
            sni: None,
        }
    }

    /// Construct an H3 backend with the given SNI.
    #[must_use]
    pub fn h3(addr: SocketAddr, sni: impl Into<String>) -> Self {
        Self {
            addr,
            proto: UpstreamProto::H3,
            sni: Some(sni.into()),
        }
    }
}

/// Multi-protocol picker returning the full [`UpstreamBackend`] descriptor.
pub trait BackendInfoPicker: Send + Sync {
    /// Next backend, or `None` if no backend is available.
    fn pick_info(&self) -> Option<UpstreamBackend>;
}

/// Adapter wrapping a single-protocol [`BackendPicker`] and tagging every pick
/// with a fixed protocol / SNI, for call sites not yet migrated to the
/// multi-proto surface.
pub struct SingleProtoPicker {
    inner: Arc<dyn BackendPicker>,
    proto: UpstreamProto,
    sni: Option<String>,
}

impl SingleProtoPicker {
    /// Wrap `picker`, tagging every pick with `proto` and an optional SNI.
    #[must_use]
    pub const fn new(
        picker: Arc<dyn BackendPicker>,
        proto: UpstreamProto,
        sni: Option<String>,
    ) -> Self {
        Self {
            inner: picker,
            proto,
            sni,
        }
    }
}

impl BackendInfoPicker for SingleProtoPicker {
    fn pick_info(&self) -> Option<UpstreamBackend> {
        let addr = self.inner.pick()?;
        Some(UpstreamBackend {
            addr,
            proto: self.proto,
            sni: self.sni.clone(),
        })
    }
}

/// Round-robin picker over a fixed `Vec<UpstreamBackend>`. Mirror of
/// [`crate::h1_proxy::RoundRobinAddrs`] for the multi-protocol path.
pub struct RoundRobinUpstreams {
    backends: Vec<UpstreamBackend>,
    counter: Mutex<usize>,
}

impl RoundRobinUpstreams {
    /// Construct a picker; returns `None` if `backends` is empty.
    #[must_use]
    pub fn new(backends: Vec<UpstreamBackend>) -> Option<Self> {
        if backends.is_empty() {
            return None;
        }
        Some(Self {
            backends,
            counter: Mutex::new(0),
        })
    }
}

impl BackendInfoPicker for RoundRobinUpstreams {
    fn pick_info(&self) -> Option<UpstreamBackend> {
        if self.backends.is_empty() {
            return None;
        }
        let idx = {
            let mut g = self.counter.lock();
            let i = *g % self.backends.len();
            *g = g.wrapping_add(1);
            i
        };
        self.backends.get(idx).cloned()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn upstream_backend_constructors_set_proto() {
        let a: SocketAddr = "127.0.0.1:80".parse().unwrap();
        assert_eq!(UpstreamBackend::h1(a).proto, UpstreamProto::H1);
        assert_eq!(UpstreamBackend::h2(a).proto, UpstreamProto::H2);
        let b3 = UpstreamBackend::h3(a, "host.test");
        assert_eq!(b3.proto, UpstreamProto::H3);
        assert_eq!(b3.sni.as_deref(), Some("host.test"));
    }

    #[test]
    fn round_robin_upstreams_cycles() {
        let a: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let b: SocketAddr = "127.0.0.1:2".parse().unwrap();
        let p =
            RoundRobinUpstreams::new(vec![UpstreamBackend::h1(a), UpstreamBackend::h2(b)]).unwrap();
        let p1 = p.pick_info().unwrap();
        let p2 = p.pick_info().unwrap();
        let p3 = p.pick_info().unwrap();
        assert_eq!(p1.addr, a);
        assert_eq!(p1.proto, UpstreamProto::H1);
        assert_eq!(p2.addr, b);
        assert_eq!(p2.proto, UpstreamProto::H2);
        assert_eq!(p3.addr, a);
    }

    #[test]
    fn round_robin_upstreams_empty_returns_none() {
        assert!(RoundRobinUpstreams::new(Vec::new()).is_none());
    }
}
