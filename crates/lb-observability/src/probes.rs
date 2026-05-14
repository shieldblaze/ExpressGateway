//! REL-2-04: process-wide liveness / readiness / startup probe state.
//!
//! [`ProbeRegistry`] is shared across the binary via [`Arc`]. The admin
//! HTTP listener reads it in the `/livez`, `/readyz`, and `/startupz`
//! handlers; the binary's `async_main` flips the bits as the process
//! transitions through bind / serve / drain.
//!
//! The state is encoded in a single [`AtomicU8`] so reads are
//! lock-free on the hot path (each scrape touches one cache line).
//! Wave 2c will wire the SIGTERM drain into
//! [`ProbeRegistry::set_draining`]; for Wave 2a we ship the data
//! structure + the HTTP endpoints, and the integration tests below
//! exercise every transition through the public API.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// Process lifecycle phase as seen by the K8s-style probes.
///
/// Transitions are strictly forward — once the process leaves
/// [`ProbeState::Starting`] it never returns. `Draining` is reached
/// from `Ready` (REL-2-02 SIGTERM path, Wave 2c).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ProbeState {
    /// Runtime is up but listeners have not yet completed bind. All
    /// three probes return 503 except `/livez` which returns 200 to
    /// confirm the process itself is alive.
    Starting = 0,
    /// All configured listeners are bound and accepting. Both
    /// `/livez` and `/readyz` return 200.
    Ready = 1,
    /// Graceful shutdown in progress (REL-2-02). `/readyz` returns
    /// 503 so the load balancer in front stops sending new traffic;
    /// `/livez` stays 200 so K8s does NOT kill the pod mid-drain.
    Draining = 2,
}

impl ProbeState {
    const fn as_byte(self) -> u8 {
        self as u8
    }

    const fn from_byte(b: u8) -> Self {
        match b {
            1 => Self::Ready,
            2 => Self::Draining,
            _ => Self::Starting,
        }
    }

    /// Body string emitted in the JSON `status` field for this state.
    #[must_use]
    pub const fn body_token(self) -> &'static str {
        match self {
            Self::Starting => "booting",
            Self::Ready => "ok",
            Self::Draining => "draining",
        }
    }
}

/// Shared registry that other crates clone an [`Arc`] of.
///
/// The expected ownership topology is:
///   - One [`Arc<ProbeRegistry>`] held by the admin HTTP service.
///   - One [`Arc<ProbeRegistry>`] held by `async_main` so it can
///     flip `Ready` after bind completes and `Draining` on SIGTERM.
#[derive(Debug)]
pub struct ProbeRegistry {
    state: AtomicU8,
}

impl Default for ProbeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProbeRegistry {
    /// Build a fresh registry in the [`ProbeState::Starting`] phase.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(ProbeState::Starting as u8),
        }
    }

    /// Convenience constructor that already wraps the registry in an
    /// [`Arc`] for share-across-tasks ergonomics.
    #[must_use]
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// Current phase. Cheap (single relaxed load).
    #[must_use]
    pub fn state(&self) -> ProbeState {
        ProbeState::from_byte(self.state.load(Ordering::Acquire))
    }

    /// Flip to [`ProbeState::Ready`]. Idempotent. No-op if the
    /// registry is already [`ProbeState::Draining`] — a draining
    /// process must never silently flip back to ready.
    pub fn set_ready(&self) {
        // CAS Starting → Ready. If we are already in Ready (idempotent
        // success) or Draining (terminal), leave the byte alone.
        let _ = self.state.compare_exchange(
            ProbeState::Starting.as_byte(),
            ProbeState::Ready.as_byte(),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// Flip to [`ProbeState::Draining`]. Idempotent. Wave 2c will
    /// call this from the SIGTERM handler in `main.rs`.
    pub fn set_draining(&self) {
        self.state
            .store(ProbeState::Draining.as_byte(), Ordering::Release);
    }

    /// `true` once at least one bind has succeeded.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        matches!(self.state(), ProbeState::Ready)
    }

    /// `true` if the runtime is alive — i.e. the process is up. Stays
    /// true through `Draining` so K8s does not yank the pod mid-drain.
    #[must_use]
    pub fn is_live(&self) -> bool {
        // Process is always live once this registry exists; only an
        // abort/exit changes that — at which point the listener no
        // longer answers anyway.
        true
    }

    /// `true` once the startup sequence has finished — same condition
    /// as `is_ready()` because we treat "all listeners bound + DNS
    /// resolved" as the single startup gate.
    #[must_use]
    pub fn is_started(&self) -> bool {
        !matches!(self.state(), ProbeState::Starting)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_in_starting_state() {
        let r = ProbeRegistry::new();
        assert_eq!(r.state(), ProbeState::Starting);
        assert!(r.is_live(), "live before ready");
        assert!(!r.is_ready());
        assert!(!r.is_started());
    }

    #[test]
    fn ready_transition_flips_flags() {
        let r = ProbeRegistry::new();
        r.set_ready();
        assert_eq!(r.state(), ProbeState::Ready);
        assert!(r.is_ready());
        assert!(r.is_started());
        assert!(r.is_live());
    }

    #[test]
    fn drain_flips_readiness_but_keeps_liveness() {
        let r = ProbeRegistry::new();
        r.set_ready();
        r.set_draining();
        assert_eq!(r.state(), ProbeState::Draining);
        assert!(!r.is_ready(), "readiness must drop during drain");
        assert!(r.is_live(), "liveness must persist through drain");
        assert!(r.is_started());
    }

    #[test]
    fn ready_after_drain_is_a_no_op() {
        let r = ProbeRegistry::new();
        r.set_ready();
        r.set_draining();
        r.set_ready();
        assert_eq!(
            r.state(),
            ProbeState::Draining,
            "draining must not flip back to ready"
        );
    }

    #[test]
    fn body_token_string_is_stable() {
        // Operators grep for these exact tokens in scrape responses.
        assert_eq!(ProbeState::Starting.body_token(), "booting");
        assert_eq!(ProbeState::Ready.body_token(), "ok");
        assert_eq!(ProbeState::Draining.body_token(), "draining");
    }
}
