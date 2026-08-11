//! CODE-2-01 — `SecurityHooks` re-export façade so the rest of `lb-l7` can
//! program against `SecurityHooks` / `SecurityReject` / `ConnPermit` without
//! naming `lb_security` at every call site. The binary constructs an
//! [`lb_security::HooksBundle`] and threads it in via `with_hooks`.
//!
//! [`NoopHooks`] is the default wired into the proxy constructors; it accepts
//! every request and connection. It is deliberately NOT `#[cfg(test)]`-gated,
//! contrary to the original brief: the production constructors need a non-test
//! default for the `hooks` field.

use std::net::IpAddr;
use std::sync::Arc;

pub use lb_security::{ConnGate, ConnPermit, SecurityHooks, SecurityReject};

/// Object-safe sibling of [`SecurityHooks`]. The upstream trait carries a
/// generic `B` on `inspect_request<B>`, which makes it NOT dyn-compatible —
/// `Arc<dyn SecurityHooks>` does not compile. The proxy hot path only inspects
/// headers + version, so this local trait pins `B = ()`.
pub trait DynSecurityHooks: Send + Sync + 'static {
    /// Inspect a parsed request before hop-by-hop strip / upstream acquire.
    /// The hot path reconstructs a `Request<()>` from the destructured parts.
    ///
    /// # Errors
    ///
    /// Returns [`SecurityReject`] on rejection (smuggle / over-cap /
    /// rate-limit / slow-handshake).
    fn inspect_request(&self, req: &http::Request<()>, peer: IpAddr) -> Result<(), SecurityReject>;

    /// Admit a new connection.
    ///
    /// # Errors
    ///
    /// Returns [`SecurityReject::OverCap`] when the per-IP /
    /// per-listener counters saturate.
    fn admit_connection(&self, peer: IpAddr) -> Result<ConnPermit, SecurityReject>;
}

/// Blanket impl bridging [`SecurityHooks`] into the object-safe
/// [`DynSecurityHooks`] surface, so any implementor (including
/// [`lb_security::HooksBundle`] and [`NoopHooks`]) is usable as
/// `Arc<dyn DynSecurityHooks>`.
impl<T: SecurityHooks> DynSecurityHooks for T {
    fn inspect_request(&self, req: &http::Request<()>, peer: IpAddr) -> Result<(), SecurityReject> {
        <T as SecurityHooks>::inspect_request(self, req, peer)
    }

    fn admit_connection(&self, peer: IpAddr) -> Result<ConnPermit, SecurityReject> {
        <T as SecurityHooks>::admit_connection(self, peer)
    }
}

/// Always-accept [`SecurityHooks`] impl, the default for proxy constructors
/// that pre-date the CODE-2-01 wire-up; replaced by
/// [`lb_security::HooksBundle`] via `H{1,2}Proxy::with_hooks`.
/// `admit_connection` admits through an internal effectively-unbounded
/// [`ConnGate`]; the permit drops harmlessly when the connection ends.
pub struct NoopHooks {
    gate: ConnGate,
}

impl Default for NoopHooks {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for NoopHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoopHooks").finish_non_exhaustive()
    }
}

impl NoopHooks {
    /// Build a [`NoopHooks`] with an effectively-unbounded [`ConnGate`] so the
    /// default proxy path never rejects an admission.
    #[must_use]
    pub fn new() -> Self {
        Self {
            gate: ConnGate::new(u32::MAX, u32::MAX, Vec::new()),
        }
    }
}

impl SecurityHooks for NoopHooks {
    fn inspect_request<B>(
        &self,
        _req: &http::Request<B>,
        _peer: IpAddr,
    ) -> Result<(), SecurityReject> {
        Ok(())
    }

    fn admit_connection(&self, peer: IpAddr) -> Result<ConnPermit, SecurityReject> {
        self.gate.admit(peer).map_err(SecurityReject::OverCap)
    }
}

/// Convenience constructor for the default [`NoopHooks`] handle.
#[must_use]
pub fn default_hooks() -> Arc<dyn DynSecurityHooks> {
    Arc::new(NoopHooks::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Request;
    use std::net::Ipv4Addr;

    #[test]
    fn noop_inspect_request_always_ok() {
        let h = NoopHooks::new();
        let req: Request<()> = Request::builder().uri("/").body(()).unwrap();
        assert!(DynSecurityHooks::inspect_request(&h, &req, Ipv4Addr::LOCALHOST.into()).is_ok());
    }

    #[test]
    fn noop_admit_connection_always_ok() {
        let h = NoopHooks::new();
        assert!(DynSecurityHooks::admit_connection(&h, Ipv4Addr::LOCALHOST.into()).is_ok());
    }

    #[test]
    fn default_hooks_returns_noop() {
        let h: Arc<dyn DynSecurityHooks> = default_hooks();
        let req: Request<()> = Request::builder().uri("/").body(()).unwrap();
        assert!(h.inspect_request(&req, Ipv4Addr::LOCALHOST.into()).is_ok());
    }
}
