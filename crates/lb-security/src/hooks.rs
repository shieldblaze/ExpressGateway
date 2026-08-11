//! Production [`SecurityHooks`] surface for the proxy hot path (SEC-2-01).
//!
//! Deliberately duplicates the shape of `lb-l7`'s `security_hooks::SecurityHooks` shim so the
//! bundle is testable ahead of the call-site rewrite — keep the two in sync.

use std::net::IpAddr;

use http::Request;

use crate::SecurityError;
use crate::conn_gate::{ConnGate, ConnPermit, OverCap};
use crate::smuggle::{SmuggleDetector, SmuggleMode};

/// Short-circuit reason for a rejected request or connection, resolved before the bridge or
/// upstream selector runs.
#[derive(Debug, thiserror::Error)]
pub enum SecurityReject {
    /// Matched a smuggling pattern. Reply 400.
    #[error("request smuggling: {0}")]
    Smuggle(#[source] SecurityError),

    /// Rate-limited after admission. Reply 429.
    #[error("rate-limited")]
    RateLimited,

    /// Handshake timed out. Reply 408, or RST if no response phase was reached.
    #[error("slow handshake")]
    SlowHandshake,

    /// Connection cap exhausted. RST WITHOUT a response — a reply here is an amplification lever.
    #[error("over-cap: {0}")]
    OverCap(#[source] OverCap),
}

/// Security decisions the proxy hot path calls into.
pub trait SecurityHooks: Send + Sync + 'static {
    /// Run every admission-time check before the bridge / upstream-acquire path.
    fn inspect_request<B>(&self, req: &Request<B>, peer: IpAddr) -> Result<(), SecurityReject>;

    /// Admit a connection; the [`ConnPermit`] must be held for the connection's whole life.
    fn admit_connection(&self, peer: IpAddr) -> Result<ConnPermit, SecurityReject>;
}

/// Production [`SecurityHooks`] impl: a [`ConnGate`] plus the smuggle mode to check under.
pub struct HooksBundle {
    gate: ConnGate,
    smuggle_mode: SmuggleMode,
}

impl HooksBundle {
    /// Build a bundle from a [`ConnGate`] and a [`SmuggleMode`].
    #[must_use]
    pub const fn new(gate: ConnGate, smuggle_mode: SmuggleMode) -> Self {
        Self { gate, smuggle_mode }
    }

    /// Borrow the inner [`ConnGate`], for metrics or to share counters across listeners.
    #[must_use]
    pub const fn gate(&self) -> &ConnGate {
        &self.gate
    }
}

impl SecurityHooks for HooksBundle {
    fn inspect_request<B>(&self, req: &Request<B>, _peer: IpAddr) -> Result<(), SecurityReject> {
        let mut pairs: Vec<(String, String)> = Vec::with_capacity(req.headers().len());
        for (name, value) in req.headers() {
            let value_str = value.to_str().unwrap_or("");
            pairs.push((name.as_str().to_string(), value_str.to_string()));
        }
        let is_h2 = matches!(req.version(), http::Version::HTTP_2);
        let mode = if is_h2 {
            SmuggleMode::H2
        } else {
            self.smuggle_mode
        };
        SmuggleDetector::check_all_mode(&pairs, mode).map_err(SecurityReject::Smuggle)?;
        Ok(())
    }

    fn admit_connection(&self, peer: IpAddr) -> Result<ConnPermit, SecurityReject> {
        self.gate.admit(peer).map_err(SecurityReject::OverCap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::{HeaderValue, Method, Request, Version};
    use std::net::Ipv4Addr;

    fn bundle() -> HooksBundle {
        let gate = ConnGate::new(8, 4, Vec::new());
        HooksBundle::new(gate, SmuggleMode::H1)
    }

    fn strict_bundle() -> HooksBundle {
        let gate = ConnGate::new(8, 4, Vec::new());
        HooksBundle::new(gate, SmuggleMode::H1Strict)
    }

    fn req_with(headers: &[(&'static str, &'static str)], version: Version) -> Request<()> {
        let mut req = Request::builder()
            .method(Method::POST)
            .uri("/")
            .version(version)
            .body(())
            .unwrap();
        for (n, v) in headers {
            req.headers_mut()
                .append(*n, HeaderValue::from_str(v).unwrap());
        }
        req
    }

    #[test]
    fn inspect_request_clean_h1_ok() {
        let b = bundle();
        let r = req_with(&[("host", "example.com")], Version::HTTP_11);
        assert!(b.inspect_request(&r, Ipv4Addr::LOCALHOST.into()).is_ok());
    }

    #[test]
    fn inspect_request_cl_te_rejected() {
        let b = bundle();
        let r = req_with(
            &[("content-length", "5"), ("transfer-encoding", "chunked")],
            Version::HTTP_11,
        );
        let err = b
            .inspect_request(&r, Ipv4Addr::LOCALHOST.into())
            .unwrap_err();
        assert!(matches!(err, SecurityReject::Smuggle(_)));
    }

    #[test]
    fn inspect_request_strict_te_rejected_only_under_strict() {
        let lenient = bundle();
        let strict = strict_bundle();
        let r = req_with(&[("transfer-encoding", "gzip, chunked")], Version::HTTP_11);
        assert!(
            lenient
                .inspect_request(&r, Ipv4Addr::LOCALHOST.into())
                .is_ok()
        );
        assert!(
            strict
                .inspect_request(&r, Ipv4Addr::LOCALHOST.into())
                .is_err()
        );
    }

    #[test]
    fn inspect_request_h2_downgrade_connection_rejected() {
        let b = bundle();
        let r = req_with(&[("connection", "keep-alive")], Version::HTTP_2);
        let err = b
            .inspect_request(&r, Ipv4Addr::LOCALHOST.into())
            .unwrap_err();
        assert!(matches!(err, SecurityReject::Smuggle(_)));
    }

    #[test]
    fn admit_connection_returns_permit() {
        let b = bundle();
        let peer: IpAddr = Ipv4Addr::LOCALHOST.into();
        let p1 = b.admit_connection(peer).unwrap();
        let p2 = b.admit_connection(peer).unwrap();
        drop(p1);
        drop(p2);
    }

    #[test]
    fn admit_connection_over_cap_rejected() {
        let gate = ConnGate::new(2, 2, Vec::new());
        let b = HooksBundle::new(gate, SmuggleMode::H1);
        let peer: IpAddr = Ipv4Addr::LOCALHOST.into();
        let _p1 = b.admit_connection(peer).unwrap();
        let _p2 = b.admit_connection(peer).unwrap();
        let err = b.admit_connection(peer).unwrap_err();
        assert!(matches!(err, SecurityReject::OverCap(_)));
    }
}
