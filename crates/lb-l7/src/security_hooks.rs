//! CODE-2-01 (lb-l7 side) — `SecurityHooks` trait shim.
//!
//! Wave-1 (commit `3dcb6f3`) added the `lb-security = { path = "../lb-security" }`
//! dependency edge to `lb-l7/Cargo.toml` without publishing any callable
//! surface. Wave-2a (commit `e36b50f`) finalised the production
//! [`SecurityHooks`] trait + [`HooksBundle`] inside `lb-security`.
//!
//! This module is the Wave-2b CODE-2-01 follow-up: a thin re-export
//! façade so the rest of `lb-l7` can program against
//! `crate::security_hooks::{SecurityHooks, SecurityReject, ConnPermit}`
//! without naming `lb_security` in every call site. Wave-2c
//! (`crates/lb/src/main.rs`) constructs an [`lb_security::HooksBundle`]
//! and threads it into [`crate::h1_proxy::H1Proxy`] /
//! [`crate::h2_proxy::H2Proxy`] via the new `with_hooks` builder.
//!
//! ## Default no-op impl
//!
//! [`NoopHooks`] is the default impl wired into the proxy constructors
//! that pre-date the CODE-2-01 fix. It accepts every request and every
//! connection. The proxy hot-path call sites all go through the trait,
//! so swapping the production [`lb_security::HooksBundle`] in via
//! `with_hooks` flips the behaviour without re-touching the call sites.
//!
//! The brief originally called for `NoopHooks` to be `#[cfg(test)]`
//! gated. We promoted it to always-public because the production
//! constructors `H1Proxy::new` / `with_multi_proto` / `H2Proxy::new` /
//! `with_security` / `with_multi_proto` (already wired from
//! `crates/lb/src/main.rs` — which Wave-2b is forbidden from
//! touching) need a non-test default for the new `hooks` field.
//! Wave-2c will call `.with_hooks(Arc::new(HooksBundle::new(...)))` and
//! the [`NoopHooks`] default falls away on the production path.

use std::net::IpAddr;
use std::sync::Arc;

pub use lb_security::{ConnGate, ConnPermit, SecurityHooks, SecurityReject};

/// Object-safe sibling of [`SecurityHooks`] used inside `lb-l7`.
///
/// The upstream [`lb_security::SecurityHooks`] trait carries a generic
/// `B` on `inspect_request<B>(&Request<B>, _)` which makes it
/// **not** dyn-compatible — `Arc<dyn SecurityHooks>` does not compile.
/// The proxy hot path only ever needs to inspect headers + version,
/// so this local trait pins `B = ()` and is therefore object-safe.
/// `lb-l7` programs against `Arc<dyn DynSecurityHooks>` everywhere;
/// Wave-2c constructs a [`HooksBundleAdapter`] around the production
/// `lb_security::HooksBundle` to satisfy the trait.
pub trait DynSecurityHooks: Send + Sync + 'static {
    /// Inspect a parsed request before hop-by-hop strip / upstream
    /// acquire. The hot path reconstructs a `Request<()>` from the
    /// destructured parts; the body is consumed separately.
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

/// Blanket impl bridging the upstream [`SecurityHooks`] trait into the
/// object-safe [`DynSecurityHooks`] surface. Any type implementing
/// [`SecurityHooks`] (including [`lb_security::HooksBundle`] and
/// [`NoopHooks`]) is automatically usable as `Arc<dyn
/// DynSecurityHooks>`.
impl<T: SecurityHooks> DynSecurityHooks for T {
    fn inspect_request(&self, req: &http::Request<()>, peer: IpAddr) -> Result<(), SecurityReject> {
        <T as SecurityHooks>::inspect_request(self, req, peer)
    }

    fn admit_connection(&self, peer: IpAddr) -> Result<ConnPermit, SecurityReject> {
        <T as SecurityHooks>::admit_connection(self, peer)
    }
}

/// Always-accept [`SecurityHooks`] impl used as the default for proxy
/// constructors that pre-date the CODE-2-01 wire-up. Replaced by
/// [`lb_security::HooksBundle`] via `H{1,2}Proxy::with_hooks` in
/// Wave-2c.
///
/// `inspect_request` returns `Ok(())` for every request;
/// `admit_connection` admits via an internal large-capacity
/// [`ConnGate`] (effectively unbounded). The permit drops harmlessly
/// when the connection ends.
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
    /// Build a [`NoopHooks`] with an effectively-unbounded
    /// [`ConnGate`] so the default proxy path never rejects an
    /// admission.
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

/// Convenience constructor for the default [`NoopHooks`] handle used by
/// pre-CODE-2-01 constructors. Wave-2c overrides via
/// `H{1,2}Proxy::with_hooks`.
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
