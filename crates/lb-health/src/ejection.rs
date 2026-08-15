//! Passive outlier ejection (G5): the admission gate every backend picker consults, and the
//! success/failure sink the L7 upstream legs feed.
//!
//! Ejection without a threshold, a re-admission path and a minimum-healthy floor is WORSE than no
//! ejection at all — one blip permanently kills a backend, or a correlated outage ejects the whole
//! fleet and the listener serves nothing. All three live here, in one place, so no picker and no
//! proxy can implement a second opinion (R12).
//!
//! Shape follows Envoy `outlier_detection` / HAProxy `observe` + `on-error`, not raw
//! eject-on-error; the per-default departures are argued at each field of [`EjectionPolicy`].

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::RwLock;

use crate::{HealthChecker, HealthStatus};

/// What one upstream attempt tells the passive health detector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptOutcome {
    /// The backend produced a response head. ANY status counts, including `5xx` — see
    /// [`UpstreamErrorClass`].
    Success,
    /// The backend did not produce one: dial, handshake, send, or the upstream deadline.
    Failure,
    /// Nothing reached the backend; the sample is DISCARDED rather than counted either way.
    NotAttempted,
}

/// How an L7 upstream leg ended. THE SINGLE DEFINITION of which upstream errors are the BACKEND's
/// fault: both `lb-l7` proxies map their local `ProxyErr` onto this enum and nothing else decides.
///
/// Application `5xx` is deliberately absent. Envoy separates `consecutive_gateway_failure` from
/// `consecutive_5xx`; only the former is implemented here, because a bad deploy returns `500` from
/// every backend at once and counting it would drive the detector straight into the floor on every
/// rollout. Connectivity faults are uncorrelated with application bugs, which is what makes them
/// safe to eject on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamErrorClass {
    /// Dial or TLS/H2/H3 handshake failure against the peer, on a connection we just opened.
    /// Backend fault.
    Transport,
    /// The upstream deadline fired. Backend fault.
    Timeout,
    /// The INBOUND request was malformed or over-cap. The client's fault, never the backend's.
    ClientRequest,
    /// The gateway is mis-wired — a backend protocol was selected with no pool behind it.
    Misconfigured,
    /// The send failed on a POOLED connection, so we cannot tell whether the request ever left this
    /// process. A cached H2 connection the peer closed while it sat idle fails exactly like a
    /// backend that is refusing work, and the pool does not report which it was.
    ///
    /// Charging this to the backend would let OUR race eject a healthy one: `N` concurrent requests
    /// sharing one stale sender fail together, producing `N` CORRELATED failures — the shape of a
    /// backend rolling restart, and enough to cross the threshold in one burst. Discarding costs one
    /// genuine signal (a backend that accepts connections but resets every stream is not ejected);
    /// [`Self::Transport`] and [`Self::Timeout`] still fire for it on the next dial.
    ///
    /// OPEN WORK ITEM — "did this attempt reach the peer?", one `reused` bit out of each upstream
    /// pool:
    /// - `lb_io::http2_pool`: `acquire_sender` returns a cached sender whenever the peer entry
    ///   `is_alive()` and does not say so. `Send`-on-reused stays unattributable; `Send`-on-fresh
    ///   becomes a real [`Self::Transport`] failure. This is ALSO the discriminator a safe upstream
    ///   retry needs, so it should be built once and consumed twice.
    /// - `lb_io::quic_pool`: `acquire` pops idle connections under the same
    ///   validate-then-use window, so the H3 upstream legs are the same SHAPE of hazard — a dead
    ///   pooled connection surfaces as "no response head", which the H3 legs currently classify as
    ///   [`Self::Transport`]. LOGGED, NOT DIAGNOSED: no evidence of the H2 failure mode there yet,
    ///   and it must not be assumed by analogy.
    Unattributable,
}

impl UpstreamErrorClass {
    /// Map the class onto the health verdict. This is the policy; call sites classify, they do not
    /// decide.
    ///
    /// [`Self::ClientRequest`] is [`AttemptOutcome::NotAttempted`], NOT a success: a success would
    /// also RESET a genuine consecutive-failure streak, so a client spraying malformed requests
    /// could hold a dying backend in rotation. Discarding the sample cannot do that.
    #[must_use]
    pub const fn outcome(self) -> AttemptOutcome {
        match self {
            Self::Transport | Self::Timeout => AttemptOutcome::Failure,
            Self::ClientRequest | Self::Misconfigured | Self::Unattributable => {
                AttemptOutcome::NotAttempted
            }
        }
    }
}

/// Tunables for [`HealthRegistry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EjectionPolicy {
    /// Master switch. When `false`, [`HealthRegistry::admits`] is a constant `true` and every
    /// record is a no-op, so the wiring stays in place with zero behavioural effect.
    pub enabled: bool,
    /// CONSECUTIVE upstream failures before a backend is ejected. Envoy's
    /// `consecutive_gateway_failure` default is 5; HAProxy's `fall` default is 3 but counts active
    /// probes spaced ~2s apart, so 3 there spans several seconds while 5 passive samples can land
    /// in milliseconds. 5 is the more conservative of the two once normalised for sample rate.
    ///
    /// A value of 1 is accepted but is a foot-gun: one RST during a rolling restart ejects a
    /// healthy backend.
    pub consecutive_failures: u32,
    /// First ejection duration; Envoy `base_ejection_time` default (30s).
    pub base_ejection: Duration,
    /// Ceiling on the backoff; Envoy `max_ejection_time` default (300s).
    pub max_ejection: Duration,
    /// Floor: the percentage of a listener's backends that must stay admitted, so a CORRELATED
    /// failure can never eject everything. Serving degraded beats serving nothing.
    ///
    /// DEPARTURE FROM ENVOY, deliberate and on the record: Envoy's analogue
    /// (`max_ejection_percent`) defaults to 10%, which with fewer than 10 backends means nothing
    /// can ever be ejected — inert for the 2-4 backend listeners this gateway actually configures.
    /// 50% (plus the absolute "never leave zero admitted" floor in [`HealthRegistry`]) gives real
    /// ejection at N=2 while still guaranteeing a shared-dependency outage cannot black-hole a
    /// listener. HAProxy's `observe` + `on-error mark-down` has no floor at all, which is precisely
    /// the "worse than nothing" behaviour this avoids.
    pub min_healthy_percent: u8,
}

impl Default for EjectionPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            consecutive_failures: 5,
            base_ejection: Duration::from_secs(30),
            max_ejection: Duration::from_secs(300),
            min_healthy_percent: 50,
        }
    }
}

/// The admission predicate a backend picker consults. Object-safe so a picker can hold
/// `Arc<dyn AdmissionGate>` and a test can substitute a deterministic double.
pub trait AdmissionGate: Send + Sync {
    /// `true` if `addr` may receive traffic right now.
    fn admits(&self, addr: SocketAddr) -> bool;
}

/// One backend's health as of a [`HealthRegistry::snapshot`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendHealth {
    /// Backend address, as the datapath knows it (resolved, not the config string).
    pub addr: SocketAddr,
    /// Threshold state machine's verdict.
    pub status: HealthStatus,
    /// `true` while the backend is ejected and its half-open deadline has not yet passed.
    pub ejected: bool,
}

#[derive(Debug)]
struct Entry {
    checker: HealthChecker,
    /// `Some(deadline)` while an ejection record stands. Past the deadline the backend is admitted
    /// again as a HALF-OPEN probe, and the next recorded outcome decides.
    ejected_until: Option<Instant>,
    /// Consecutive ejection rounds, driving the backoff. Cleared by a success.
    rounds: u32,
}

impl Entry {
    fn new(consecutive_failures: u32) -> Self {
        Self {
            // One success re-admits (Envoy half-open parity); the failure side is the policy knob.
            checker: HealthChecker::new(1, consecutive_failures),
            ejected_until: None,
            rounds: 0,
        }
    }
}

#[derive(Debug)]
struct Inner {
    policy: EjectionPolicy,
    entries: HashMap<SocketAddr, Entry>,
}

/// Per-listener passive health state: threshold, ejection, half-open re-admission and the
/// minimum-healthy floor.
///
/// Keyed by RESOLVED [`SocketAddr`], because that is the only backend identity the datapath ever
/// holds. Lives for the process, ACROSS config reloads — rebuilding it on SIGHUP would let an
/// operator's reload loop silently disable ejection.
#[derive(Debug)]
pub struct HealthRegistry {
    inner: RwLock<Inner>,
    ejections_total: AtomicU64,
    readmissions_total: AtomicU64,
    ejections_suppressed_total: AtomicU64,
}

impl HealthRegistry {
    /// Registry over `backends`, every entry starting [`HealthStatus::Unknown`] and admitted.
    #[must_use]
    pub fn new(policy: EjectionPolicy, backends: &[SocketAddr]) -> Self {
        let mut entries = HashMap::with_capacity(backends.len());
        for addr in backends {
            entries.insert(*addr, Entry::new(policy.consecutive_failures));
        }
        Self {
            inner: RwLock::new(Inner { policy, entries }),
            ejections_total: AtomicU64::new(0),
            readmissions_total: AtomicU64::new(0),
            ejections_suppressed_total: AtomicU64::new(0),
        }
    }

    /// Apply a new backend set: add the new as `Unknown`, drop the gone, PRESERVE every survivor's
    /// state. Called on a config reload — a re-resolved DNS answer legitimately changes the key set.
    pub fn reseed(&self, backends: &[SocketAddr]) {
        let mut guard = self.inner.write();
        let inner = &mut *guard;
        let threshold = inner.policy.consecutive_failures;
        inner.entries.retain(|addr, _| backends.contains(addr));
        for addr in backends {
            inner
                .entries
                .entry(*addr)
                .or_insert_with(|| Entry::new(threshold));
        }
    }

    /// Swap the policy live. A changed [`EjectionPolicy::consecutive_failures`] rebuilds each
    /// checker (the threshold is baked in at construction): standing ejections survive, only the
    /// in-progress failure streak resets.
    pub fn set_policy(&self, policy: EjectionPolicy) {
        let mut guard = self.inner.write();
        let inner = &mut *guard;
        if inner.policy.consecutive_failures != policy.consecutive_failures {
            for entry in inner.entries.values_mut() {
                entry.checker = HealthChecker::new(1, policy.consecutive_failures);
            }
        }
        inner.policy = policy;
    }

    /// The one entry point the L7 legs call; [`AttemptOutcome::NotAttempted`] records nothing.
    pub fn record(&self, addr: SocketAddr, outcome: AttemptOutcome) {
        match outcome {
            AttemptOutcome::Success => self.record_success(addr),
            AttemptOutcome::Failure => self.record_failure(addr),
            AttemptOutcome::NotAttempted => {}
        }
    }

    /// Record a successful attempt. Clears any standing ejection unconditionally — a backend that
    /// is demonstrably serving must never stay ejected, even if the success came from a request
    /// picked before the ejection landed.
    pub fn record_success(&self, addr: SocketAddr) {
        let readmitted = {
            let mut guard = self.inner.write();
            if !guard.policy.enabled {
                return;
            }
            let Some(entry) = guard.entries.get_mut(&addr) else {
                return;
            };
            entry.checker.record_success();
            if entry.ejected_until.take().is_some() {
                entry.rounds = 0;
                true
            } else {
                false
            }
        };
        if readmitted {
            self.readmissions_total.fetch_add(1, Ordering::Relaxed);
            tracing::info!(
                backend = %addr,
                "passive health: backend re-admitted after a successful half-open probe"
            );
        }
    }

    /// Record a failed attempt, ejecting only once the CONSECUTIVE-failure threshold is crossed and
    /// only if the minimum-healthy floor still permits it.
    pub fn record_failure(&self, addr: SocketAddr) {
        let action = {
            let mut guard = self.inner.write();
            if !guard.policy.enabled {
                return;
            }
            let inner = &mut *guard;
            let policy = inner.policy;
            let now = Instant::now();
            // O(n) over ONE listener's backends, and only on a failure — never on the admit path.
            let ejected_now = count_ejected(&inner.entries, now);
            let total = inner.entries.len();
            let Some(entry) = inner.entries.get_mut(&addr) else {
                return;
            };
            entry.checker.record_failure();
            match entry.ejected_until {
                // The half-open probe failed: this entry was already paid for against the floor at
                // its first ejection, so back off further without re-checking it.
                Some(deadline) if now >= deadline => {
                    entry.rounds = entry.rounds.saturating_add(1);
                    let window = backoff(policy, entry.rounds);
                    entry.ejected_until = Some(now + window);
                    FailureAction::ReEjected(window)
                }
                // Still inside the ejection window — an in-flight request that raced the ejection.
                Some(_) => FailureAction::None,
                None if entry.checker.status() != HealthStatus::Unhealthy => FailureAction::None,
                None => {
                    if can_eject(total, ejected_now, policy) {
                        entry.rounds = 1;
                        let window = backoff(policy, 1);
                        entry.ejected_until = Some(now + window);
                        FailureAction::Ejected(window)
                    } else {
                        FailureAction::Suppressed {
                            ejected: ejected_now,
                            total,
                        }
                    }
                }
            }
        };

        match action {
            FailureAction::None => {}
            FailureAction::Ejected(window) => {
                self.ejections_total.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    backend = %addr,
                    ejected_for_ms = millis(window),
                    "passive health: backend EJECTED after consecutive upstream failures"
                );
            }
            FailureAction::ReEjected(window) => {
                self.ejections_total.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    backend = %addr,
                    ejected_for_ms = millis(window),
                    "passive health: half-open probe failed — backend re-ejected with backoff"
                );
            }
            // A suppressed ejection is a LOUD event: the operator is now serving traffic to a
            // backend known to be failing, and only the floor is keeping the listener alive.
            FailureAction::Suppressed { ejected, total } => {
                self.ejections_suppressed_total
                    .fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    backend = %addr,
                    ejected,
                    total,
                    min_healthy_percent = u32::from(self.inner.read().policy.min_healthy_percent),
                    "passive health: ejection SUPPRESSED by the minimum-healthy floor — \
                     backend is failing but stays in rotation (serving degraded)"
                );
            }
        }
    }

    /// Threshold state of `addr`; [`HealthStatus::Unknown`] for an address the registry never saw.
    #[must_use]
    pub fn status(&self, addr: SocketAddr) -> HealthStatus {
        self.inner
            .read()
            .entries
            .get(&addr)
            .map_or(HealthStatus::Unknown, |e| e.checker.status())
    }

    /// Backends NOT currently admitted. A half-open backend is admitted, so it is not counted here
    /// and does not hold down the floor — one definition of "ejected", used by both.
    #[must_use]
    pub fn ejected_count(&self) -> usize {
        let guard = self.inner.read();
        count_ejected(&guard.entries, Instant::now())
    }

    /// Number of backends the registry tracks.
    #[must_use]
    pub fn backend_count(&self) -> usize {
        self.inner.read().entries.len()
    }

    /// Monotonic count of ejections (including re-ejections after a failed half-open probe).
    #[must_use]
    pub fn ejections_total(&self) -> u64 {
        self.ejections_total.load(Ordering::Relaxed)
    }

    /// Monotonic count of re-admissions.
    #[must_use]
    pub fn readmissions_total(&self) -> u64 {
        self.readmissions_total.load(Ordering::Relaxed)
    }

    /// Monotonic count of ejections REFUSED by the minimum-healthy floor.
    #[must_use]
    pub fn ejections_suppressed_total(&self) -> u64 {
        self.ejections_suppressed_total.load(Ordering::Relaxed)
    }

    /// Per-backend view for the metrics pump; order is unspecified.
    #[must_use]
    pub fn snapshot(&self) -> Vec<BackendHealth> {
        let guard = self.inner.read();
        let now = Instant::now();
        guard
            .entries
            .iter()
            .map(|(addr, entry)| BackendHealth {
                addr: *addr,
                status: entry.checker.status(),
                ejected: entry.ejected_until.is_some_and(|d| now < d),
            })
            .collect()
    }
}

impl AdmissionGate for HealthRegistry {
    /// R3: `Unknown` and `Healthy` both admit, and an untracked address admits. A process that has
    /// never crossed the failure threshold against any backend therefore has a constant-`true`
    /// gate, which is what makes routing byte-identical to the pre-ejection build.
    fn admits(&self, addr: SocketAddr) -> bool {
        let guard = self.inner.read();
        if !guard.policy.enabled {
            return true;
        }
        guard.entries.get(&addr).is_none_or(|entry| {
            entry
                .ejected_until
                .is_none_or(|deadline| Instant::now() >= deadline)
        })
    }
}

/// What [`HealthRegistry::record_failure`] decided, resolved before the lock is released so the
/// counter bumps and logs happen outside it.
enum FailureAction {
    None,
    Ejected(Duration),
    ReEjected(Duration),
    Suppressed { ejected: usize, total: usize },
}

/// `tracing` has no `Value` impl for `u128`, so a duration field must be narrowed before logging.
fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

fn count_ejected(entries: &HashMap<SocketAddr, Entry>, now: Instant) -> usize {
    entries
        .values()
        .filter(|e| e.ejected_until.is_some_and(|deadline| now < deadline))
        .count()
}

/// The minimum-healthy floor, evaluated at EJECTION time rather than at pick time: at pick time the
/// answer would depend on evaluation order and flap request-to-request, while here it is a stable
/// property of the registry and is directly assertable.
fn can_eject(total: usize, ejected: usize, policy: EjectionPolicy) -> bool {
    if total == 0 {
        return false;
    }
    let ejectable_pct = usize::from(100_u8.saturating_sub(policy.min_healthy_percent));
    let max_ejectable = total.saturating_mul(ejectable_pct) / 100;
    let after = ejected.saturating_add(1);
    // The percentage AND an absolute floor: `min_healthy_percent = 0` must still not eject the
    // last backend.
    after <= max_ejectable && total.saturating_sub(after) >= 1
}

fn backoff(policy: EjectionPolicy, rounds: u32) -> Duration {
    // Exponential with a cap. Envoy's is linear (`base × consecutive_ejections`); both reach the
    // same `max_ejection` ceiling, and exponential backs a FLAPPING backend off sooner.
    let shift = rounds.saturating_sub(1).min(16);
    let window = policy
        .base_ejection
        .checked_mul(2_u32.saturating_pow(shift))
        .unwrap_or(policy.max_ejection);
    window.min(policy.max_ejection)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(n: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], n))
    }

    fn fast_policy() -> EjectionPolicy {
        EjectionPolicy {
            base_ejection: Duration::from_millis(50),
            max_ejection: Duration::from_millis(200),
            ..EjectionPolicy::default()
        }
    }

    #[test]
    fn fresh_registry_admits_everything() {
        let reg = HealthRegistry::new(EjectionPolicy::default(), &[addr(1), addr(2)]);
        assert!(reg.admits(addr(1)));
        assert!(reg.admits(addr(2)));
        // An address the registry never saw must admit too, or a reload race would 502.
        assert!(reg.admits(addr(9)));
        assert_eq!(reg.ejected_count(), 0);
    }

    #[test]
    fn disabled_policy_never_ejects() {
        let policy = EjectionPolicy {
            enabled: false,
            ..fast_policy()
        };
        let reg = HealthRegistry::new(policy, &[addr(1), addr(2)]);
        for _ in 0..50 {
            reg.record_failure(addr(1));
        }
        assert!(reg.admits(addr(1)));
        assert_eq!(reg.ejections_total(), 0);
    }

    #[test]
    fn client_request_and_misconfig_are_discarded() {
        assert_eq!(
            UpstreamErrorClass::ClientRequest.outcome(),
            AttemptOutcome::NotAttempted
        );
        assert_eq!(
            UpstreamErrorClass::Misconfigured.outcome(),
            AttemptOutcome::NotAttempted
        );
        assert_eq!(
            UpstreamErrorClass::Transport.outcome(),
            AttemptOutcome::Failure
        );
        assert_eq!(
            UpstreamErrorClass::Timeout.outcome(),
            AttemptOutcome::Failure
        );
        assert_eq!(
            UpstreamErrorClass::Unattributable.outcome(),
            AttemptOutcome::NotAttempted,
            "a pooled send that may never have left this process is not the backend's fault"
        );

        // And the discard must not clear a real streak.
        let reg = HealthRegistry::new(fast_policy(), &[addr(1), addr(2)]);
        for _ in 0..4 {
            reg.record_failure(addr(1));
        }
        reg.record(addr(1), UpstreamErrorClass::ClientRequest.outcome());
        reg.record_failure(addr(1));
        assert!(
            !reg.admits(addr(1)),
            "a discarded sample must not reset the streak"
        );
    }

    #[test]
    fn reseed_preserves_survivors_and_drops_the_gone() {
        let reg = HealthRegistry::new(fast_policy(), &[addr(1), addr(2), addr(3)]);
        for _ in 0..5 {
            reg.record_failure(addr(1));
        }
        assert!(!reg.admits(addr(1)));

        reg.reseed(&[addr(1), addr(2), addr(4)]);
        assert_eq!(reg.backend_count(), 3);
        assert!(
            !reg.admits(addr(1)),
            "a survivor keeps its ejection across a reload"
        );
        assert!(
            reg.admits(addr(4)),
            "a new backend starts Unknown and admitted"
        );
    }

    #[test]
    fn policy_swap_rebuilds_checkers_but_keeps_ejections() {
        let reg = HealthRegistry::new(fast_policy(), &[addr(1), addr(2)]);
        for _ in 0..5 {
            reg.record_failure(addr(1));
        }
        assert!(!reg.admits(addr(1)));
        reg.set_policy(EjectionPolicy {
            consecutive_failures: 9,
            ..fast_policy()
        });
        assert!(
            !reg.admits(addr(1)),
            "a policy swap must not silently re-admit"
        );
    }

    #[test]
    fn backoff_is_capped() {
        let policy = EjectionPolicy {
            base_ejection: Duration::from_secs(30),
            max_ejection: Duration::from_secs(300),
            ..EjectionPolicy::default()
        };
        assert_eq!(backoff(policy, 1), Duration::from_secs(30));
        assert_eq!(backoff(policy, 2), Duration::from_secs(60));
        assert_eq!(backoff(policy, 4), Duration::from_secs(240));
        assert_eq!(backoff(policy, 5), Duration::from_secs(300));
        assert_eq!(backoff(policy, 99), Duration::from_secs(300));
    }

    #[test]
    fn can_eject_floor_arithmetic() {
        let p50 = EjectionPolicy::default();
        // N=1: never, whatever the percentage.
        assert!(!can_eject(1, 0, p50));
        // N=2 at 50%: exactly one may go.
        assert!(can_eject(2, 0, p50));
        assert!(!can_eject(2, 1, p50));
        // N=3 at 50%: floor(1.5) = 1.
        assert!(can_eject(3, 0, p50));
        assert!(!can_eject(3, 1, p50));
        // N=4 at 50%: two.
        assert!(can_eject(4, 1, p50));
        assert!(!can_eject(4, 2, p50));
        // 0% still refuses to empty the pool.
        let p0 = EjectionPolicy {
            min_healthy_percent: 0,
            ..EjectionPolicy::default()
        };
        assert!(can_eject(2, 0, p0));
        assert!(!can_eject(2, 1, p0));
        assert!(!can_eject(1, 0, p0));
    }
}
