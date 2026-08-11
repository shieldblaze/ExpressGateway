//! Per-IP and per-listener concurrent-connection cap (SEC-2-04).
//!
//! [`ConnGate::admit`] runs at accept time and never blocks; on [`OverCap`] the listener must RST
//! the socket WITHOUT writing a response, or the cap itself becomes an amplification lever.
//!
//! The per-listener counter uses AcqRel rather than Relaxed per SEC-2-16: it gates a security
//! decision, so the consume edge has to observe every prior decrement.
//!
//! The trusted-CIDR field is carried but NOT matched — deferred per `audit/deferred.md` L-002.

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use dashmap::DashMap;

/// Reason an [`admit`](ConnGate::admit) call was refused.
#[derive(Debug, thiserror::Error)]
pub enum OverCap {
    /// Per-listener counter at `listener_cap`.
    #[error("listener cap exhausted ({0})")]
    Listener(u32),

    /// Per-IP counter at `per_ip_cap`.
    #[error("per-IP cap exhausted for {addr} ({count})")]
    PerIp {
        /// Source IP whose counter is saturated.
        addr: IpAddr,
        /// Current count (== `per_ip_cap`).
        count: u32,
    },
}

/// Trusted-CIDR prefix. Stored verbatim — nothing matches against it yet (L-002).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpNet {
    /// Prefix base address.
    pub addr: IpAddr,
    /// Prefix length in bits.
    pub prefix_len: u8,
}

impl IpNet {
    /// Construct from raw address + prefix length.
    #[must_use]
    pub const fn new(addr: IpAddr, prefix_len: u8) -> Self {
        Self { addr, prefix_len }
    }
}

/// Shared state behind an `Arc` so [`ConnPermit::drop`] can decrement without borrowing the gate.
struct GateInner {
    per_ip: DashMap<IpAddr, u32>,
    per_listener: AtomicU32,
    per_ip_cap: u32,
    listener_cap: u32,
    trusted_cidrs: Vec<IpNet>,
}

/// Per-IP / per-listener gate; cheap to clone, shared across accept loops that share caps.
#[derive(Clone)]
pub struct ConnGate {
    inner: Arc<GateInner>,
}

/// RAII handle from [`ConnGate::admit`]; dropping releases both counter slots.
pub struct ConnPermit {
    inner: Arc<GateInner>,
    peer: IpAddr,
}

impl std::fmt::Debug for ConnPermit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnPermit")
            .field("peer", &self.peer)
            .finish_non_exhaustive()
    }
}

impl ConnGate {
    /// Build a gate. `trusted_cidrs` is stored but not matched (L-002); pass an empty `Vec`.
    #[must_use]
    pub fn new(listener_cap: u32, per_ip_cap: u32, trusted_cidrs: Vec<IpNet>) -> Self {
        Self {
            inner: Arc::new(GateInner {
                per_ip: DashMap::new(),
                per_listener: AtomicU32::new(0),
                per_ip_cap,
                listener_cap,
                trusted_cidrs,
            }),
        }
    }

    /// Per-listener cap.
    #[must_use]
    pub fn listener_cap(&self) -> u32 {
        self.inner.listener_cap
    }

    /// Per-IP cap.
    #[must_use]
    pub fn per_ip_cap(&self) -> u32 {
        self.inner.per_ip_cap
    }

    /// Per-listener count — a metrics snapshot, not authoritative under concurrent admits.
    #[must_use]
    pub fn current_listener_count(&self) -> u32 {
        self.inner.per_listener.load(Ordering::Acquire)
    }

    /// Current count for a peer. `0` if no outstanding permits.
    #[must_use]
    pub fn current_peer_count(&self, peer: IpAddr) -> u32 {
        self.inner.per_ip.get(&peer).map_or(0, |v| *v)
    }

    /// Trusted-CIDR list (deferred per L-002).
    #[must_use]
    pub fn trusted_cidrs(&self) -> &[IpNet] {
        &self.inner.trusted_cidrs
    }

    /// Admit a connection from `peer`.
    ///
    /// The per-IP overflow path MUST roll the per-listener counter back (it was already bumped);
    /// without the rollback a sustained over-cap stream silently erodes the listener cap.
    pub fn admit(&self, peer: IpAddr) -> Result<ConnPermit, OverCap> {
        let mut cur = self.inner.per_listener.load(Ordering::Acquire);
        loop {
            if cur >= self.inner.listener_cap {
                return Err(OverCap::Listener(cur));
            }
            match self.inner.per_listener.compare_exchange_weak(
                cur,
                cur + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(observed) => cur = observed,
            }
        }

        let mut entry = self.inner.per_ip.entry(peer).or_insert(0);
        if *entry >= self.inner.per_ip_cap {
            let count = *entry;
            drop(entry);
            self.inner.per_listener.fetch_sub(1, Ordering::AcqRel);
            return Err(OverCap::PerIp { addr: peer, count });
        }
        *entry += 1;
        drop(entry);

        Ok(ConnPermit {
            inner: Arc::clone(&self.inner),
            peer,
        })
    }
}

impl Drop for ConnPermit {
    fn drop(&mut self) {
        let mut should_gc = false;
        if let Some(mut entry) = self.inner.per_ip.get_mut(&self.peer) {
            if *entry > 0 {
                *entry -= 1;
            }
            if *entry == 0 {
                should_gc = true;
            }
        }
        if should_gc {
            // Racing an admit here is safe — it re-inserts via `entry().or_insert(0)`.
            self.inner.per_ip.remove_if(&self.peer, |_, v| *v == 0);
        }
        let prev = self.inner.per_listener.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(prev > 0, "per_listener counter underflow");
    }
}
