//! Upstream backend dispatch for the L7 proxies (PROTO-001): a per-backend
//! protocol selector so a dial can reach H1, H2 or H3.

use std::net::SocketAddr;
use std::sync::Arc;

use lb_health::AdmissionGate;
use parking_lot::Mutex;

use crate::h1_proxy::BackendPicker;

/// Upstream wire protocol for a backend dial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpstreamProto {
    /// HTTP/1.1 over plain TCP (`Backend::protocol` `"tcp"` or `"h1"`).
    H1,
    /// HTTP/2 over plain TCP, via [`lb_io::http2_pool::Http2Pool`].
    H2,
    /// HTTP/3 over QUIC, via [`lb_io::quic_pool::QuicUpstreamPool`].
    H3,
}

/// Resolved upstream backend: address, wire protocol, and an SNI for H3.
#[derive(Debug, Clone)]
pub struct UpstreamBackend {
    /// Peer address.
    pub addr: SocketAddr,
    /// Wire protocol to speak to this backend.
    pub proto: UpstreamProto,
    /// SNI: required for `H3`, ignored for `H1`/`H2`.
    pub sni: Option<String>,
}

impl UpstreamBackend {
    /// Plain H1 backend, no SNI.
    #[must_use]
    pub const fn h1(addr: SocketAddr) -> Self {
        Self {
            addr,
            proto: UpstreamProto::H1,
            sni: None,
        }
    }

    /// Plain H2 backend, no SNI.
    #[must_use]
    pub const fn h2(addr: SocketAddr) -> Self {
        Self {
            addr,
            proto: UpstreamProto::H2,
            sni: None,
        }
    }

    /// H3 backend with the given SNI.
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

/// Tags every pick from a single-protocol [`BackendPicker`] with a fixed
/// protocol / SNI, for call sites predating the multi-proto surface.
pub struct SingleProtoPicker {
    inner: Arc<dyn BackendPicker>,
    proto: UpstreamProto,
    sni: Option<String>,
}

impl SingleProtoPicker {
    /// Wrap `picker`, tagging every pick with `proto` and an SNI.
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

/// Round-robin picker over a fixed `Vec<UpstreamBackend>`.
pub struct RoundRobinUpstreams {
    backends: Vec<UpstreamBackend>,
    counter: Mutex<usize>,
}

impl RoundRobinUpstreams {
    /// `None` if `backends` is empty.
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

/// G5 — THE health filter for the L7 datapath. Every L7 pick funnels through
/// [`BackendInfoPicker`], so wrapping it here single-sources ejection across every picker that
/// exists or will exist (R12); filtering inside the pickers would be one copy of the policy each.
///
/// Filtering the backend LIST instead was rejected: `lb_balancer`'s pickers return an INDEX, so
/// removing entries renumbers every index — which for Maglev and ring-hash remaps every consistent
/// hash key, moving all traffic rather than only the ejected backend's share.
///
/// **R3 — byte-identical routing while healthy.** [`Self::pick_info`] returns inside its FIRST loop
/// iteration whenever the gate admits, so the inner picker is advanced exactly once per pick and
/// its round-robin SEQUENCE (not merely its backend set) is unchanged from the unwrapped build.
/// A fresh [`lb_health::HealthRegistry`] admits every address, so this holds until something
/// actually crosses the failure threshold.
///
/// Exhausting `max_attempts` FAILS OPEN — the last pick is returned even though it is ejected.
/// Serving degraded beats serving nothing, and it bounds any bug in the gate to today's behaviour.
pub struct HealthFilteredPicker {
    inner: Arc<dyn BackendInfoPicker>,
    gate: Arc<dyn AdmissionGate>,
    /// Bounded by the backend count: one pass over the rotation is enough to find an admitted
    /// backend if one exists, and a picker with a stuck counter cannot spin.
    max_attempts: usize,
}

impl HealthFilteredPicker {
    /// Wrap `inner`, skipping backends `gate` refuses. `backend_count` bounds the retry loop.
    #[must_use]
    pub fn new(
        inner: Arc<dyn BackendInfoPicker>,
        gate: Arc<dyn AdmissionGate>,
        backend_count: usize,
    ) -> Self {
        Self {
            inner,
            gate,
            max_attempts: backend_count.max(1),
        }
    }
}

impl BackendInfoPicker for HealthFilteredPicker {
    fn pick_info(&self) -> Option<UpstreamBackend> {
        let mut last = None;
        for _ in 0..self.max_attempts {
            let backend = self.inner.pick_info()?;
            if self.gate.admits(backend.addr) {
                return Some(backend);
            }
            last = Some(backend);
        }
        last
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

    /// Counts inner picks so the R3 "exactly one inner advance per outer pick" property is
    /// asserted directly rather than inferred from an output sequence.
    struct CountingPicker {
        inner: RoundRobinUpstreams,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl BackendInfoPicker for CountingPicker {
        fn pick_info(&self) -> Option<UpstreamBackend> {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.inner.pick_info()
        }
    }

    /// Admits everything except an explicit deny-list.
    struct DenyList(Vec<SocketAddr>);

    impl AdmissionGate for DenyList {
        fn admits(&self, addr: SocketAddr) -> bool {
            !self.0.contains(&addr)
        }
    }

    fn triple() -> (SocketAddr, SocketAddr, SocketAddr) {
        (
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:2".parse().unwrap(),
            "127.0.0.1:3".parse().unwrap(),
        )
    }

    fn counting(a: SocketAddr, b: SocketAddr, c: SocketAddr) -> Arc<CountingPicker> {
        Arc::new(CountingPicker {
            inner: RoundRobinUpstreams::new(vec![
                UpstreamBackend::h1(a),
                UpstreamBackend::h1(b),
                UpstreamBackend::h1(c),
            ])
            .unwrap(),
            calls: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    /// R3 PROOF (a): while everything is admitted, the wrapper consumes exactly one inner pick per
    /// outer pick, so the inner round-robin counter advances identically to the unwrapped build.
    /// Would FAIL on any design that pre-filters the list or probes ahead.
    #[test]
    fn healthy_gate_consumes_exactly_one_inner_pick() {
        let (a, b, c) = triple();
        let counter = counting(a, b, c);
        let picker = HealthFilteredPicker::new(
            Arc::clone(&counter) as Arc<dyn BackendInfoPicker>,
            Arc::new(DenyList(Vec::new())),
            3,
        );
        for _ in 0..30 {
            assert!(picker.pick_info().is_some());
        }
        assert_eq!(
            counter.calls.load(std::sync::atomic::Ordering::Relaxed),
            30,
            "a healthy gate must not consume extra inner picks"
        );
    }

    /// R3 PROOF (b): the emitted SEQUENCE, not merely the set, matches the unwrapped picker.
    #[test]
    fn healthy_gate_preserves_the_round_robin_sequence() {
        let (a, b, c) = triple();
        let bare = RoundRobinUpstreams::new(vec![
            UpstreamBackend::h1(a),
            UpstreamBackend::h1(b),
            UpstreamBackend::h1(c),
        ])
        .unwrap();
        let wrapped = HealthFilteredPicker::new(
            Arc::new(
                RoundRobinUpstreams::new(vec![
                    UpstreamBackend::h1(a),
                    UpstreamBackend::h1(b),
                    UpstreamBackend::h1(c),
                ])
                .unwrap(),
            ),
            Arc::new(DenyList(Vec::new())),
            3,
        );
        let bare_seq: Vec<SocketAddr> = (0..30)
            .filter_map(|_| bare.pick_info())
            .map(|b| b.addr)
            .collect();
        let wrapped_seq: Vec<SocketAddr> = (0..30)
            .filter_map(|_| wrapped.pick_info())
            .map(|b| b.addr)
            .collect();
        assert_eq!(bare_seq, wrapped_seq);
    }

    /// NON-VACUITY for the two proofs above: with one backend denied the sequences MUST diverge.
    /// Without this, `assert_eq!(bare_seq, wrapped_seq)` would also pass against a wrapper that
    /// does nothing at all, and neither proof would be worth anything.
    #[test]
    fn denied_backend_makes_the_sequence_diverge() {
        let (a, b, c) = triple();
        let bare = RoundRobinUpstreams::new(vec![
            UpstreamBackend::h1(a),
            UpstreamBackend::h1(b),
            UpstreamBackend::h1(c),
        ])
        .unwrap();
        let wrapped = HealthFilteredPicker::new(
            Arc::new(
                RoundRobinUpstreams::new(vec![
                    UpstreamBackend::h1(a),
                    UpstreamBackend::h1(b),
                    UpstreamBackend::h1(c),
                ])
                .unwrap(),
            ),
            Arc::new(DenyList(vec![b])),
            3,
        );
        let bare_seq: Vec<SocketAddr> = (0..30)
            .filter_map(|_| bare.pick_info())
            .map(|x| x.addr)
            .collect();
        let wrapped_seq: Vec<SocketAddr> = (0..30)
            .filter_map(|_| wrapped.pick_info())
            .map(|x| x.addr)
            .collect();
        assert_ne!(
            bare_seq, wrapped_seq,
            "the harness must be able to see a difference"
        );
        assert!(
            !wrapped_seq.contains(&b),
            "the denied backend must not be picked: {wrapped_seq:?}"
        );
        assert!(wrapped_seq.contains(&a) && wrapped_seq.contains(&c));
    }

    /// FAIL-OPEN backstop: when every backend is denied the picker still returns one rather than
    /// `None`, because `None` becomes a `502` at `h1_proxy`/`h2_proxy` — serving nothing.
    #[test]
    fn all_denied_fails_open_instead_of_returning_none() {
        let (a, b, c) = triple();
        let picker = HealthFilteredPicker::new(
            Arc::new(
                RoundRobinUpstreams::new(vec![
                    UpstreamBackend::h1(a),
                    UpstreamBackend::h1(b),
                    UpstreamBackend::h1(c),
                ])
                .unwrap(),
            ),
            Arc::new(DenyList(vec![a, b, c])),
            3,
        );
        assert!(
            picker.pick_info().is_some(),
            "a fully-denied set must degrade, not 502"
        );
    }

    /// An empty inner picker must still short-circuit to `None` rather than looping.
    #[test]
    fn empty_inner_returns_none() {
        struct Empty;
        impl BackendInfoPicker for Empty {
            fn pick_info(&self) -> Option<UpstreamBackend> {
                None
            }
        }
        let picker = HealthFilteredPicker::new(Arc::new(Empty), Arc::new(DenyList(Vec::new())), 3);
        assert!(picker.pick_info().is_none());
    }
}
