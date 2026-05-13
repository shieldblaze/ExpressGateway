//! Production [`SecurityHooks`] surface for the proxy hot path.
//!
//! Wave-2a SEC-2-01 (API half) — paired with `lb-l7`'s
//! `security_hooks::SecurityHooks` trait shim (Wave-2b CODE-2-01 follow-up
//! in `crates/lb-l7/src/security_hooks.rs`). `lb-l7` cannot publish the
//! trait yet without the call-site rewrite in `h{1,2}_proxy.rs`; in the
//! meantime this crate publishes a structurally-equivalent trait so the
//! production [`HooksBundle`] is unit-testable end-to-end and the
//! Wave-2b refactor only has to flip the import.
//!
//! ## Wire shape (final, mirrored by Wave-2b)
//!
//! ```ignore
//! pub trait SecurityHooks: Send + Sync + 'static {
//!     fn inspect_request<B>(
//!         &self,
//!         req: &http::Request<B>,
//!         peer: std::net::IpAddr,
//!     ) -> Result<(), SecurityReject>;
//!
//!     fn admit_connection(
//!         &self,
//!         peer: std::net::IpAddr,
//!     ) -> Result<ConnPermit, SecurityReject>;
//! }
//! ```
//!
//! [`SecurityReject`] enumerates the short-circuit outcomes that the
//! proxy converts to a 400 (smuggling), 429 (rate-limited), 408
//! (slow-handshake), or RST-without-response (over-cap). [`ConnPermit`]
//! is an RAII handle returned by [`ConnGate::admit`] — dropping it
//! decrements the per-IP and per-listener counters.

use std::net::IpAddr;

use http::Request;

use crate::SecurityError;
use crate::conn_gate::{ConnGate, ConnPermit, OverCap};
use crate::smuggle::{SmuggleDetector, SmuggleMode};

/// Short-circuit reason for a rejected request or connection.
///
/// The proxy hot path converts these to an HTTP status (or a TCP RST
/// for `OverCap`) without consulting the bridge or upstream selector.
#[derive(Debug, thiserror::Error)]
pub enum SecurityReject {
    /// Request matched a smuggling pattern (CL+TE, duplicate CL with
    /// differing values, H2-downgrade with forbidden hop-by-hop
    /// headers, strict-TE violation, ...). Reply with HTTP 400.
    #[error("request smuggling: {0}")]
    Smuggle(#[source] SecurityError),

    /// Connection was admitted but later upgraded to rate-limited.
    /// Reply with HTTP 429.
    #[error("rate-limited")]
    RateLimited,

    /// TLS / HTTP handshake exceeded the timeout. Reply with HTTP 408
    /// or RST depending on which phase was active.
    #[error("slow handshake")]
    SlowHandshake,

    /// Per-IP or per-listener connection cap exhausted. RST the socket
    /// without writing a response (no amplification surface).
    #[error("over-cap: {0}")]
    OverCap(#[source] OverCap),
}

/// Trait the proxy hot path calls into for security decisions.
///
/// `Send + Sync + 'static` so it can live behind `Arc<dyn SecurityHooks>`
/// in the proxy's shared state. The Wave-2b shim in `lb-l7` republishes
/// this exact shape; the production [`HooksBundle`] in this crate is the
/// one impl callers actually wire up.
pub trait SecurityHooks: Send + Sync + 'static {
    /// Inspect a parsed request and run all admission-time security
    /// checks before the bridge / upstream-acquire path runs.
    ///
    /// # Errors
    ///
    /// Returns a [`SecurityReject`] describing the rejection class.
    fn inspect_request<B>(&self, req: &Request<B>, peer: IpAddr) -> Result<(), SecurityReject>;

    /// Admit a new connection. The returned [`ConnPermit`] holds the
    /// per-IP / per-listener counters for the lifetime of the
    /// connection; drop releases them.
    ///
    /// # Errors
    ///
    /// Returns [`SecurityReject::OverCap`] when either counter is
    /// saturated.
    fn admit_connection(&self, peer: IpAddr) -> Result<ConnPermit, SecurityReject>;
}

/// Production [`SecurityHooks`] impl.
///
/// Bundles the [`ConnGate`] (per-IP / per-listener cap) and a strict-TE
/// flag for the [`SmuggleDetector::check_all_mode`] call-site. Wave-2b
/// will pass an `Arc<HooksBundle>` into the proxy constructor; tests
/// (and `NoopHooks` in the Wave-2b shim) construct it directly.
pub struct HooksBundle {
    gate: ConnGate,
    smuggle_mode: SmuggleMode,
}

impl HooksBundle {
    /// Build a bundle from a constructed [`ConnGate`] and smuggle mode.
    ///
    /// `smuggle_mode` is `SmuggleMode::H1` for default (lenient TE) or
    /// `SmuggleMode::H1Strict` to reject any non-`chunked` codec in
    /// `Transfer-Encoding` per SEC-2-15 matrix.
    #[must_use]
    pub const fn new(gate: ConnGate, smuggle_mode: SmuggleMode) -> Self {
        Self { gate, smuggle_mode }
    }

    /// Borrow the inner [`ConnGate`]. Useful for metrics or sharing the
    /// counters across listener loops.
    #[must_use]
    pub const fn gate(&self) -> &ConnGate {
        &self.gate
    }
}

impl SecurityHooks for HooksBundle {
    fn inspect_request<B>(&self, req: &Request<B>, _peer: IpAddr) -> Result<(), SecurityReject> {
        // Collect headers into the existing `(String, String)` shape the
        // detector uses. The hot-path version (Wave-2b) will adapt the
        // detector to read directly from `http::HeaderMap` to avoid the
        // allocation; for the API surface, the slice shape is what the
        // existing unit tests cover.
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
        // Lenient: final codec is chunked, accepted.
        assert!(
            lenient
                .inspect_request(&r, Ipv4Addr::LOCALHOST.into())
                .is_ok()
        );
        // Strict: any codec beyond `chunked` rejected.
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
