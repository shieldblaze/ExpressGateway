//! Process-wide liveness / readiness / startup probe state, in one [`AtomicU8`] so scrapes stay
//! lock-free.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// Lifecycle phase behind the probes. Transitions are STRICTLY FORWARD — nothing returns to
/// `Starting`, and nothing leaves `Draining`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ProbeState {
    /// Up but not bound; only `/livez` returns 200.
    Starting = 0,
    /// Bound and accepting; `/livez` and `/readyz` both 200.
    Ready = 1,
    /// Draining: `/readyz` 503 to stop new traffic, but `/livez` stays 200 or K8s kills the pod
    /// mid-drain.
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

/// Shared probe state: one `Arc` in the admin service, one in `async_main` to flip the phase.
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

    /// [`Self::new`] pre-wrapped in an [`Arc`].
    #[must_use]
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// Current phase. Cheap (single relaxed load).
    #[must_use]
    pub fn state(&self) -> ProbeState {
        ProbeState::from_byte(self.state.load(Ordering::Acquire))
    }

    /// Flip to `Ready`. NO-OP while `Draining` — a draining process must never read ready again.
    pub fn set_ready(&self) {
        // CAS only from Starting; Ready is idempotent and Draining is terminal.
        let _ = self.state.compare_exchange(
            ProbeState::Starting.as_byte(),
            ProbeState::Ready.as_byte(),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// Flip to `Draining`; idempotent.
    pub fn set_draining(&self) {
        self.state
            .store(ProbeState::Draining.as_byte(), Ordering::Release);
    }

    /// `true` once at least one bind has succeeded.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        matches!(self.state(), ProbeState::Ready)
    }

    /// True whenever the process is up, INCLUDING while draining.
    #[must_use]
    pub fn is_live(&self) -> bool {
        // If the process were not live, the listener would not be answering.
        true
    }

    /// True once startup finished; the same gate as `is_ready()`.
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
