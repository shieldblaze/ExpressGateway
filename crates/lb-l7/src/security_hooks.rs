//! CODE-2-01 — `SecurityHooks` re-export façade for `lb-l7`.
//!
//! [`NoopHooks`] is the default wired into the proxy constructors and accepts
//! everything. Deliberately NOT `#[cfg(test)]`-gated: the production
//! constructors need a non-test default for the `hooks` field.

use std::net::IpAddr;
use std::sync::Arc;

pub use lb_security::{ConnGate, ConnPermit, SecurityHooks, SecurityReject};

/// Object-safe sibling of [`SecurityHooks`]: the upstream trait's generic `B`
/// on `inspect_request<B>` makes `Arc<dyn SecurityHooks>` uncompilable, so this
/// pins `B = ()` (the hot path only inspects headers + version).
pub trait DynSecurityHooks: Send + Sync + 'static {
    /// Inspect a parsed request before hop-by-hop strip / upstream acquire.
    ///
    /// # Errors
    /// [`SecurityReject`] on rejection.
    fn inspect_request(&self, req: &http::Request<()>, peer: IpAddr) -> Result<(), SecurityReject>;

    /// Admit a new connection.
    ///
    /// # Errors
    /// [`SecurityReject::OverCap`] when the per-IP / per-listener counters
    /// saturate.
    fn admit_connection(&self, peer: IpAddr) -> Result<ConnPermit, SecurityReject>;
}

/// Blanket impl bridging [`SecurityHooks`] into [`DynSecurityHooks`].
impl<T: SecurityHooks> DynSecurityHooks for T {
    fn inspect_request(&self, req: &http::Request<()>, peer: IpAddr) -> Result<(), SecurityReject> {
        <T as SecurityHooks>::inspect_request(self, req, peer)
    }

    fn admit_connection(&self, peer: IpAddr) -> Result<ConnPermit, SecurityReject> {
        <T as SecurityHooks>::admit_connection(self, peer)
    }
}

/// Always-accept [`SecurityHooks`] impl — the proxy-constructor default until
/// [`lb_security::HooksBundle`] is wired via `H{1,2}Proxy::with_hooks`.
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
    /// Build a [`NoopHooks`] whose [`ConnGate`] never rejects an admission.
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
