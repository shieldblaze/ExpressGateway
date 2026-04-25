//! L4 XDP/eBPF data plane for TCP and UDP load balancing.
//!
//! Provides userspace simulation of the L4 XDP data plane. Real eBPF programs
//! cannot be tested in CI, so we simulate the conntrack table, Maglev consistent
//! hashing, and hot-swap behavior.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    missing_docs
)]
#![allow(clippy::pedantic, clippy::nursery)]
#![cfg_attr(
    test,
    allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)
)]

use std::collections::{HashMap, VecDeque};

// Pillar 4a: real aya-based userspace loader, Linux-only. The standalone
// BPF program it loads lives in crates/lb-l4-xdp/ebpf/ and is compiled
// out-of-workspace via scripts/build-xdp.sh.
#[cfg(target_os = "linux")]
pub mod loader;

/// Pillar 4b-2 userspace simulation of the BPF extensions.
///
/// Covers 802.1Q VLAN stripping, IPv6 conntrack lookups, LPM-trie ACL
/// matching, and RFC 1624 incremental checksum updates. The real in-kernel
/// code lives under `crates/lb-l4-xdp/ebpf/src/main.rs`; this module is the
/// CI-safe functional spec those routines must satisfy.
pub mod sim;

/// The compiled BPF ELF produced by `scripts/build-xdp.sh`.
///
/// `build.rs` emits `cfg(lb_xdp_elf)` when `src/lb_xdp.bin` is present, so
/// this constant exists only when a real ELF has been built and committed.
///
/// Pillar 4b-1 produces this artifact (~3 KiB) and tracks it as a binary
/// blob alongside the source. `XdpLoader::load_from_bytes(LB_XDP_ELF)` is
/// the supported entry point.
///
/// The bytes are emitted with 8-byte alignment so downstream `object`-crate
/// parsers that cast ELF headers through `from_bytes` (alignment-checked)
/// do not reject them.
#[cfg(lb_xdp_elf)]
pub const LB_XDP_ELF: &[u8] = {
    #[repr(C, align(8))]
    struct Aligned<B: ?Sized>(B);
    static ALIGNED: &Aligned<[u8; include_bytes!("lb_xdp.bin").len()]> =
        &Aligned(*include_bytes!("lb_xdp.bin"));
    &ALIGNED.0
};

/// Default maximum number of conntrack entries (1 million flows).
const DEFAULT_CONNTRACK_MAX_ENTRIES: usize = 1_000_000;

/// Errors from the XDP data plane simulation.
#[derive(Debug, thiserror::Error)]
pub enum XdpError {
    /// No backends configured.
    #[error("no backends configured")]
    NoBackends,

    /// Table size must be a prime number greater than zero.
    #[error("invalid table size: must be a prime number > 0")]
    InvalidTableSize,

    /// Lookup failed because the table is empty.
    #[error("lookup on empty table")]
    EmptyTable,

    /// Internal error.
    #[error("internal error: {0}")]
    Internal(String),
}

/// A flow 5-tuple key for connection tracking.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct FlowKey {
    /// Source IP address (as `u32` for IPv4 simulation).
    pub src_addr: u32,
    /// Destination IP address.
    pub dst_addr: u32,
    /// Source port.
    pub src_port: u16,
    /// Destination port.
    pub dst_port: u16,
    /// Protocol number (6 = TCP, 17 = UDP).
    pub protocol: u8,
}

/// Simulated conntrack table -- maps flow 5-tuples to backend indices.
///
/// Supports a maximum capacity with FIFO eviction: when the table is full,
/// the oldest entry is evicted to make room for new insertions.
#[derive(Debug)]
pub struct ConntrackTable {
    entries: HashMap<FlowKey, usize>,
    insert_order: VecDeque<FlowKey>,
    max_entries: usize,
}

impl ConntrackTable {
    /// Create an empty conntrack table with the default capacity (1M entries).
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            insert_order: VecDeque::new(),
            max_entries: DEFAULT_CONNTRACK_MAX_ENTRIES,
        }
    }

    /// Create an empty conntrack table with the given maximum capacity.
    #[must_use]
    pub fn with_capacity(max_entries: usize) -> Self {
        Self {
            entries: HashMap::new(),
            insert_order: VecDeque::new(),
            max_entries: max_entries.max(1),
        }
    }

    /// Look up the backend index for a given flow.
    #[must_use]
    pub fn lookup(&self, flow: &FlowKey) -> Option<usize> {
        self.entries.get(flow).copied()
    }

    /// Insert or update a flow-to-backend mapping.
    ///
    /// If the table is at capacity, the oldest entry is evicted first.
    pub fn insert(&mut self, flow: FlowKey, backend_idx: usize) {
        // If the flow already exists, update in-place without touching capacity.
        if let std::collections::hash_map::Entry::Occupied(mut e) = self.entries.entry(flow.clone())
        {
            e.insert(backend_idx);
            return;
        }

        // Evict the oldest entry if at capacity.
        while self.entries.len() >= self.max_entries {
            if let Some(oldest) = self.insert_order.pop_front() {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }

        self.entries.insert(flow.clone(), backend_idx);
        self.insert_order.push_back(flow);
    }

    /// Remove a flow entry, returning the backend index if it existed.
    pub fn remove(&mut self, flow: &FlowKey) -> Option<usize> {
        let removed = self.entries.remove(flow);
        if removed.is_some() {
            self.insert_order.retain(|k| k != flow);
        }
        removed
    }

    /// Number of active flow entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the table has no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for ConntrackTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Check whether `n` is a prime number.
const fn is_prime(n: usize) -> bool {
    if n < 2 {
        return false;
    }
    if n < 4 {
        return true;
    }
    if n % 2 == 0 || n % 3 == 0 {
        return false;
    }
    let mut i = 5;
    while i * i <= n {
        if n % i == 0 || n % (i + 2) == 0 {
            return false;
        }
        i += 6;
    }
    true
}

/// Maglev consistent hash table for L4 load balancing.
///
/// This is a standalone BPF-map-style table, independent of the `lb-balancer`
/// crate, suitable for simulating XDP data-plane lookups.
#[derive(Debug)]
pub struct MaglevTable {
    table: Vec<usize>,
    backend_count: usize,
}

impl MaglevTable {
    /// Build a new Maglev lookup table for the given backend names.
    ///
    /// # Errors
    ///
    /// Returns `XdpError::NoBackends` if `backends` is empty, or
    /// `XdpError::InvalidTableSize` if `table_size` is zero or not prime.
    pub fn new(backends: &[String], table_size: usize) -> Result<Self, XdpError> {
        if backends.is_empty() {
            return Err(XdpError::NoBackends);
        }
        if !is_prime(table_size) {
            return Err(XdpError::InvalidTableSize);
        }

        let table = Self::populate(backends, table_size);
        Ok(Self {
            table,
            backend_count: backends.len(),
        })
    }

    /// Look up the backend index for a given hash key.
    ///
    /// # Errors
    ///
    /// Returns `XdpError::EmptyTable` if the table is empty.
    #[allow(clippy::cast_possible_truncation)]
    pub fn lookup(&self, key: u64) -> Result<usize, XdpError> {
        if self.table.is_empty() {
            return Err(XdpError::EmptyTable);
        }
        let slot = (key as usize) % self.table.len();
        self.table
            .get(slot)
            .copied()
            .ok_or_else(|| XdpError::Internal("table slot out of range".into()))
    }

    /// Number of backends this table was built for.
    #[must_use]
    pub const fn backend_count(&self) -> usize {
        self.backend_count
    }

    /// Compute offset and skip for a backend name.
    #[allow(clippy::cast_possible_truncation)]
    fn permutation(name: &str, table_size: usize) -> (usize, usize) {
        let h1 = Self::hash_str(name, 0);
        let h2 = Self::hash_str(name, 0x9e37_79b9_7f4a_7c15);

        let offset = (h1 as usize) % table_size;
        let skip = if table_size > 1 {
            ((h2 as usize) % (table_size - 1)) + 1
        } else {
            1
        };
        (offset, skip)
    }

    /// Simple multiply-shift string hash (same algorithm as lb-balancer).
    fn hash_str(s: &str, seed: u64) -> u64 {
        let mut h = seed;
        for byte in s.bytes() {
            h = h
                .wrapping_mul(0x517c_c1b7_2722_0a95)
                .wrapping_add(u64::from(byte));
        }
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 33;
        h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        h ^= h >> 33;
        h
    }

    /// Populate the Maglev lookup table.
    fn populate(backends: &[String], table_size: usize) -> Vec<usize> {
        let n = backends.len();
        let perms: Vec<(usize, usize)> = backends
            .iter()
            .map(|b| Self::permutation(b, table_size))
            .collect();

        let mut next = vec![0usize; n];
        let mut table = vec![usize::MAX; table_size];
        let mut filled = 0usize;

        while filled < table_size {
            for i in 0..n {
                let (offset, skip) = match perms.get(i) {
                    Some(p) => *p,
                    None => continue,
                };

                let mut c = match next.get(i) {
                    Some(v) => *v,
                    None => continue,
                };
                let mut slot = (offset + c * skip) % table_size;

                // Advance until we find an empty slot.
                while table.get(slot).copied() != Some(usize::MAX) {
                    c += 1;
                    slot = (offset + c * skip) % table_size;
                }

                if let Some(entry) = table.get_mut(slot) {
                    *entry = i;
                }
                if let Some(n_i) = next.get_mut(i) {
                    *n_i = c + 1;
                }
                filled += 1;

                if filled >= table_size {
                    break;
                }
            }
        }

        table
    }
}

/// Atomic backend table swap manager.
///
/// Old flows keep their pinned backend via conntrack; new flows use the new
/// Maglev table. After a swap, stale conntrack entries pointing to out-of-range
/// backend indices are detected and evicted on the next lookup.
#[derive(Debug)]
pub struct HotSwapManager {
    conntrack: ConntrackTable,
    current_table: MaglevTable,
}

impl HotSwapManager {
    /// Create a new hot-swap manager with the given initial backend set.
    ///
    /// # Errors
    ///
    /// Returns `XdpError` if the Maglev table cannot be built.
    pub fn new(backends: &[String], table_size: usize) -> Result<Self, XdpError> {
        let table = MaglevTable::new(backends, table_size)?;
        Ok(Self {
            conntrack: ConntrackTable::new(),
            current_table: table,
        })
    }

    /// Swap to a new backend set. Existing conntrack entries are preserved,
    /// so in-flight flows continue to reach their original backend.
    ///
    /// # Errors
    ///
    /// Returns `XdpError` if the new Maglev table cannot be built.
    pub fn swap_backends(
        &mut self,
        new_backends: &[String],
        table_size: usize,
    ) -> Result<(), XdpError> {
        let new_table = MaglevTable::new(new_backends, table_size)?;
        self.current_table = new_table;
        Ok(())
    }

    /// Route a flow: check conntrack first, then fall back to the Maglev table.
    ///
    /// If the conntrack entry points to a backend index that is out of range
    /// for the current table (e.g. after a swap to fewer backends), the stale
    /// entry is removed and the flow is re-routed via Maglev.
    ///
    /// If the flow is new, the Maglev lookup result is inserted into the
    /// conntrack table for future lookups.
    ///
    /// # Errors
    ///
    /// Returns `XdpError` if the Maglev lookup fails.
    pub fn route_flow(&mut self, flow: FlowKey, hash_key: u64) -> Result<usize, XdpError> {
        if let Some(idx) = self.conntrack.lookup(&flow) {
            if idx < self.current_table.backend_count() {
                return Ok(idx);
            }
            // Stale entry -- remove and re-route via Maglev.
            self.conntrack.remove(&flow);
        }

        let idx = self.current_table.lookup(hash_key)?;
        self.conntrack.insert(flow, idx);
        Ok(idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conntrack_insert_lookup_remove() {
        let mut ct = ConntrackTable::new();
        assert!(ct.is_empty());

        let flow = FlowKey {
            src_addr: 1,
            dst_addr: 2,
            src_port: 100,
            dst_port: 80,
            protocol: 6,
        };

        ct.insert(flow.clone(), 5);
        assert_eq!(ct.len(), 1);
        assert_eq!(ct.lookup(&flow), Some(5));

        let removed = ct.remove(&flow);
        assert_eq!(removed, Some(5));
        assert!(ct.is_empty());
        assert_eq!(ct.lookup(&flow), None);
    }

    #[test]
    fn conntrack_default() {
        let ct = ConntrackTable::default();
        assert!(ct.is_empty());
    }

    #[test]
    fn conntrack_eviction_at_capacity() {
        let mut ct = ConntrackTable::with_capacity(3);

        let make_flow = |src_port: u16| FlowKey {
            src_addr: 1,
            dst_addr: 2,
            src_port,
            dst_port: 80,
            protocol: 6,
        };

        ct.insert(make_flow(1), 0);
        ct.insert(make_flow(2), 1);
        ct.insert(make_flow(3), 2);
        assert_eq!(ct.len(), 3);

        // Inserting a 4th entry should evict the oldest (src_port=1).
        ct.insert(make_flow(4), 3);
        assert_eq!(ct.len(), 3);
        assert_eq!(
            ct.lookup(&make_flow(1)),
            None,
            "oldest entry should be evicted"
        );
        assert_eq!(ct.lookup(&make_flow(2)), Some(1));
        assert_eq!(ct.lookup(&make_flow(3)), Some(2));
        assert_eq!(ct.lookup(&make_flow(4)), Some(3));
    }

    #[test]
    fn conntrack_update_existing_does_not_evict() {
        let mut ct = ConntrackTable::with_capacity(2);

        let flow_a = FlowKey {
            src_addr: 1,
            dst_addr: 2,
            src_port: 100,
            dst_port: 80,
            protocol: 6,
        };
        let flow_b = FlowKey {
            src_addr: 1,
            dst_addr: 2,
            src_port: 200,
            dst_port: 80,
            protocol: 6,
        };

        ct.insert(flow_a.clone(), 0);
        ct.insert(flow_b.clone(), 1);
        assert_eq!(ct.len(), 2);

        // Update flow_a -- should NOT evict anything.
        ct.insert(flow_a.clone(), 99);
        assert_eq!(ct.len(), 2);
        assert_eq!(ct.lookup(&flow_a), Some(99));
        assert_eq!(ct.lookup(&flow_b), Some(1));
    }

    #[test]
    fn is_prime_checks() {
        assert!(!is_prime(0));
        assert!(!is_prime(1));
        assert!(is_prime(2));
        assert!(is_prime(3));
        assert!(!is_prime(4));
        assert!(is_prime(5));
        assert!(!is_prime(6));
        assert!(is_prime(7));
        assert!(is_prime(65537));
        assert!(!is_prime(65536));
        assert!(!is_prime(100));
    }

    #[test]
    fn maglev_non_prime_table_size_rejected() {
        let backends = vec!["b1".into()];
        let result = MaglevTable::new(&backends, 100);
        assert!(result.is_err());

        let result = MaglevTable::new(&backends, 65536);
        assert!(result.is_err());
    }

    #[test]
    fn maglev_consistency() {
        let backends: Vec<String> = (0..3).map(|i| format!("b{i}")).collect();
        let table = MaglevTable::new(&backends, 65537).unwrap();

        for key in 0..100u64 {
            let a = table.lookup(key).unwrap();
            let b = table.lookup(key).unwrap();
            assert_eq!(a, b);
        }
    }

    #[test]
    fn maglev_no_backends_error() {
        let result = MaglevTable::new(&[], 65537);
        assert!(result.is_err());
    }

    #[test]
    fn maglev_zero_table_size_error() {
        let backends = vec!["b1".into()];
        let result = MaglevTable::new(&backends, 0);
        assert!(result.is_err());
    }

    #[test]
    fn maglev_distribution() {
        let backends: Vec<String> = (0..3).map(|i| format!("backend-{i}")).collect();
        let table = MaglevTable::new(&backends, 65537).unwrap();
        assert_eq!(table.backend_count(), 3);

        let mut counts = [0u32; 3];
        for i in 0..3000u64 {
            let idx = table.lookup(i).unwrap();
            counts[idx] += 1;
        }
        for count in &counts {
            assert!(*count > 0, "each backend should get some traffic");
        }
    }

    #[test]
    fn hotswap_preserves_conntrack() {
        let backends: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        let mut mgr = HotSwapManager::new(&backends, 65537).unwrap();

        let flow = FlowKey {
            src_addr: 10,
            dst_addr: 20,
            src_port: 5000,
            dst_port: 443,
            protocol: 6,
        };

        let original = mgr.route_flow(flow.clone(), 999).unwrap();

        let new_backends: Vec<String> = vec!["x".into(), "y".into(), "z".into(), "w".into()];
        mgr.swap_backends(&new_backends, 65537).unwrap();

        let after = mgr.route_flow(flow, 999).unwrap();
        assert_eq!(original, after);
    }

    #[test]
    fn hotswap_evicts_stale_conntrack_after_shrink() {
        // Start with 5 backends so conntrack entries can point to indices 0..4.
        let backends: Vec<String> = (0..5).map(|i| format!("backend-{i}")).collect();
        let mut mgr = HotSwapManager::new(&backends, 65537).unwrap();

        let flow = FlowKey {
            src_addr: 10,
            dst_addr: 20,
            src_port: 5000,
            dst_port: 443,
            protocol: 6,
        };

        let original_idx = mgr.route_flow(flow.clone(), 42).unwrap();

        // Swap to only 2 backends. Any conntrack entry with idx >= 2 is stale.
        let fewer_backends: Vec<String> = vec!["new-a".into(), "new-b".into()];
        mgr.swap_backends(&fewer_backends, 65537).unwrap();

        let rerouted_idx = mgr.route_flow(flow, 42).unwrap();

        // The rerouted index must be valid for the new (smaller) backend set.
        assert!(
            rerouted_idx < 2,
            "rerouted index must be < new backend count"
        );

        // If the original was already in-range, it should be unchanged; if it
        // was out-of-range, it must have been re-routed via Maglev.
        if original_idx >= 2 {
            // Was stale, must have been re-routed.
            assert!(rerouted_idx < 2);
        } else {
            // Was still valid, conntrack should have returned it as-is.
            assert_eq!(rerouted_idx, original_idx);
        }
    }
}
