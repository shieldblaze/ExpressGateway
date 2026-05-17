//! Per-IP and per-listener concurrent-connection cap (SEC-2-04).
//!
//! [`ConnGate::admit`] is the synchronous accept-time check the
//! listener loop calls right after `TcpStream::accept` returns. On
//! success it produces a [`ConnPermit`], an RAII handle that bumps
//! both the per-IP counter and the per-listener counter; dropping the
//! permit releases both. On failure the gate returns [`OverCap`] and
//! the listener loop is expected to RST the socket without writing a
//! response — preventing the cap from being used as an amplification
//! lever.
//!
//! Wave-2c (`crates/lb/src/main.rs`) inserts the call. This crate
//! ships the API + tests. The Wave-2a deliverable is the public
//! surface; the trusted-CIDR allowlist field is accepted so the
//! Wave-2c call site doesn't have to bump the constructor signature
//! later, but the actual CIDR-prefix match is deferred to
//! `audit/deferred.md` per L-002.
//!
//! Counter semantics
//! -----------------
//!
//! * `per_listener: AtomicU32` — single atomic, AcqRel on the
//!   success-path `compare_exchange` per SEC-2-16 (the gate counter
//!   is a security-gating value, so the consume edge must observe
//!   all previous decrements). Saturated at `listener_cap`.
//! * `per_ip: DashMap<IpAddr, u32>` — concurrent map; mutation under
//!   the bucket's lock. Entries are GC'd lazily on permit drop
//!   (decrement to 0 removes the key).
//!
//! Failure mode under flood
//! -----------------------
//!
//! [`ConnGate::admit`] never blocks. Overflow returns immediately
//! with [`OverCap::Listener`] or [`OverCap::PerIp`] so the caller can
//! close the socket on the same scheduler tick.

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

/// Placeholder type for the deferred trusted-CIDR allowlist.
///
/// Holds the prefix and prefix length verbatim. The actual prefix
/// match is deferred per `audit/deferred.md` L-002. Round-4 ships
/// the field on [`ConnGate`] so callers (Wave-2c `lb/src/main.rs`)
/// can compile against the final constructor signature; once L-002
/// lands the match becomes a single method change here.
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

/// Internal shared state. Held behind `Arc` so [`ConnPermit::drop`]
/// can decrement counters without borrowing the gate.
struct GateInner {
    per_ip: DashMap<IpAddr, u32>,
    per_listener: AtomicU32,
    per_ip_cap: u32,
    listener_cap: u32,
    trusted_cidrs: Vec<IpNet>,
}

/// Per-IP / per-listener gate.
///
/// Cheap to clone (`Arc` newtype). Shared across all listener accept
/// loops that should observe the same caps.
#[derive(Clone)]
pub struct ConnGate {
    inner: Arc<GateInner>,
}

/// RAII handle returned by [`ConnGate::admit`]. Dropping releases
/// both the per-IP and per-listener counter slots.
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
    /// Build a new gate.
    ///
    /// * `listener_cap` — maximum concurrent permits across all peers.
    /// * `per_ip_cap` — maximum concurrent permits per source IP.
    /// * `trusted_cidrs` — exemption list; **match deferred** per
    ///   L-002. Pass an empty `Vec` if unused.
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

    /// Current per-listener count (snapshot; non-authoritative under
    /// concurrent admits). Useful for metrics.
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

    /// Admit a new connection from `peer`.
    ///
    /// Bumps the per-listener counter via AcqRel `compare_exchange`
    /// (security-gating per SEC-2-16). On success, bumps the per-IP
    /// counter under the bucket lock. On per-IP overflow, rolls the
    /// per-listener counter back so cap accounting stays consistent.
    ///
    /// # Errors
    ///
    /// Returns [`OverCap::Listener`] or [`OverCap::PerIp`].
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
            // Best-effort GC: race with another admit on this IP is
            // safe; the next admit will re-insert via `entry().or_insert(0)`.
            self.inner.per_ip.remove_if(&self.peer, |_, v| *v == 0);
        }
        let prev = self.inner.per_listener.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(prev > 0, "per_listener counter underflow");
    }
}
