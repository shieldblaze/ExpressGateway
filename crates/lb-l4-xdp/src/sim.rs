//! Pillar 4b-2 userspace simulation of the BPF data-plane extensions:
//! 802.1Q VLAN stripping, IPv6 conntrack, LPM-trie ACL, and RFC 1624
//! incremental checksum updates.
//!
//! The simulation does not replace the in-kernel program — it's a
//! CI-safe functional spec. The BPF source is authoritative; these
//! routines are what we can exercise without `CAP_BPF`.
//!
//! All helpers are byte-order-agnostic in their test contracts; the
//! production BPF code operates on network-byte-order packet data.

#![allow(
    clippy::module_name_repetitions,
    clippy::missing_const_for_fn,
    reason = "sim module mirrors ebpf code; const-ness is irrelevant for tests"
)]

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, Ipv6Addr};

// ---------------------------------------------------------------------------
// VLAN stripping.
// ---------------------------------------------------------------------------

/// Ethernet + optional VLAN header shape the simulation understands.
/// Raw wire bytes of a frame prefix (14 or 18 bytes) — exactly what the
/// BPF program sees at `ctx.data()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrippedFrame {
    /// Destination MAC.
    pub dst: [u8; 6],
    /// Source MAC.
    pub src: [u8; 6],
    /// Ether-type after any VLAN tag was stripped (big-endian u16).
    pub ether_type: u16,
    /// VLAN ID if one was present, else `None`.
    pub vlan_id: Option<u16>,
    /// Offset (in bytes) where the L3 header begins. 14 for untagged,
    /// 18 for single-tag.
    pub l3_offset: usize,
}

/// Parse Ethernet + optional single 802.1Q VLAN tag, returning the
/// stripped frame prefix or `None` on bounds failure.
#[must_use]
pub fn strip_vlan(frame: &[u8]) -> Option<StrippedFrame> {
    if frame.len() < 14 {
        return None;
    }
    let dst: [u8; 6] = frame.get(0..6)?.try_into().ok()?;
    let src: [u8; 6] = frame.get(6..12)?.try_into().ok()?;
    let outer_type = u16::from_be_bytes([*frame.get(12)?, *frame.get(13)?]);

    if outer_type == 0x8100 {
        if frame.len() < 18 {
            return None;
        }
        let tci = u16::from_be_bytes([*frame.get(14)?, *frame.get(15)?]);
        let inner_type = u16::from_be_bytes([*frame.get(16)?, *frame.get(17)?]);
        Some(StrippedFrame {
            dst,
            src,
            ether_type: inner_type,
            vlan_id: Some(tci & 0x0FFF),
            l3_offset: 18,
        })
    } else {
        Some(StrippedFrame {
            dst,
            src,
            ether_type: outer_type,
            vlan_id: None,
            l3_offset: 14,
        })
    }
}

// ---------------------------------------------------------------------------
// IPv6 conntrack simulation.
// ---------------------------------------------------------------------------

/// IPv6 5-tuple flow key — userspace model of `FlowKeyV6`.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct FlowKeyV6 {
    /// Source IPv6 address.
    pub src: Ipv6Addr,
    /// Destination IPv6 address.
    pub dst: Ipv6Addr,
    /// Source port (network byte order is the BPF convention; tests
    /// may use either as long as both sides agree).
    pub src_port: u16,
    /// Destination port.
    pub dst_port: u16,
    /// IP protocol number (TCP=6, UDP=17).
    pub protocol: u8,
}

/// What the simulation's BPF decision function returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimAction {
    /// `XDP_PASS` — to kernel / userspace.
    Pass,
    /// `XDP_DROP` — ACL denied.
    Drop,
    /// `XDP_TX` — rewrite-and-transmit. The opaque backend index is
    /// attached so assertions can verify correct steering.
    Tx {
        /// Backend table index selected by the conntrack lookup.
        backend_idx: u32,
    },
}

/// IPv6 conntrack table mirror. `BTreeMap` for deterministic iteration;
/// semantics (insert / lookup / `backend_idx`) match the BPF map.
#[derive(Debug, Default)]
pub struct ConntrackV6 {
    entries: BTreeMap<(Ipv6Addr, Ipv6Addr, u16, u16, u8), u32>,
}

impl ConntrackV6 {
    /// Create an empty v6 conntrack table.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Insert a `(flow → backend_idx)` mapping.
    pub fn insert(&mut self, key: &FlowKeyV6, backend_idx: u32) {
        self.entries.insert(
            (key.src, key.dst, key.src_port, key.dst_port, key.protocol),
            backend_idx,
        );
    }

    /// Simulate the BPF decision: on a hit, return `Tx` with the pinned
    /// backend; otherwise `Pass`.
    #[must_use]
    pub fn decide(&self, key: &FlowKeyV6) -> SimAction {
        self.entries
            .get(&(key.src, key.dst, key.src_port, key.dst_port, key.protocol))
            .map_or(SimAction::Pass, |idx| SimAction::Tx { backend_idx: *idx })
    }
}

// ---------------------------------------------------------------------------
// LPM trie ACL simulation.
// ---------------------------------------------------------------------------

/// Simple userspace LPM trie over IPv4 /CIDR entries. Not performance-
/// critical (tests only); the real in-kernel structure is
/// `BPF_MAP_TYPE_LPM_TRIE`.
#[derive(Debug, Default)]
pub struct AclTrie {
    entries: Vec<(u32, u8)>,
}

impl AclTrie {
    /// Create an empty ACL trie.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Add a deny rule. `addr` is the network address; `prefix_len` is
    /// the CIDR prefix length in bits (0..=32).
    pub fn deny(&mut self, addr: Ipv4Addr, prefix_len: u8) {
        let mask = if prefix_len == 0 {
            0
        } else {
            u32::MAX << (32 - prefix_len.min(32))
        };
        let net = u32::from(addr) & mask;
        self.entries.push((net, prefix_len.min(32)));
    }

    /// Look up the longest prefix matching `addr`; return the prefix
    /// length on match, `None` otherwise.
    #[must_use]
    pub fn longest_match(&self, addr: Ipv4Addr) -> Option<u8> {
        let a = u32::from(addr);
        self.entries
            .iter()
            .filter_map(|(net, plen)| {
                let mask = if *plen == 0 {
                    0
                } else {
                    u32::MAX << (32 - *plen)
                };
                ((a & mask) == *net).then_some(*plen)
            })
            .max()
    }

    /// True iff `addr` is covered by some deny rule.
    #[must_use]
    pub fn denies(&self, addr: Ipv4Addr) -> bool {
        self.longest_match(addr).is_some()
    }
}

// ---------------------------------------------------------------------------
// RFC 1624 incremental checksum helpers (userspace mirrors of the
// BPF code in `ebpf/src/main.rs`).
// ---------------------------------------------------------------------------

/// Fold a 32-bit one's-complement sum to 16 bits.
#[must_use]
#[inline]
#[allow(
    clippy::cast_possible_truncation,
    reason = "post-fold value fits in u16"
)]
pub fn fold32(mut sum: u32) -> u16 {
    sum = (sum & 0xFFFF) + (sum >> 16);
    sum = (sum & 0xFFFF) + (sum >> 16);
    sum as u16
}

/// RFC 1624 equation 3: HC' = ~(~HC + ~m + m').
#[must_use]
#[inline]
pub fn csum16_update(old_csum: u16, old_field: u16, new_field: u16) -> u16 {
    let sum: u32 = u32::from(!old_csum) + u32::from(!old_field) + u32::from(new_field);
    !fold32(sum)
}

/// Compute a full one's-complement checksum over `bytes`. Used by the
/// RFC 1624 property test to sanity-check that the incremental update
/// agrees with a fresh sum.
#[must_use]
pub fn full_checksum(bytes: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut chunks = bytes.chunks_exact(2);
    for chunk in &mut chunks {
        let (Some(hi), Some(lo)) = (chunk.first().copied(), chunk.get(1).copied()) else {
            break;
        };
        sum = sum.wrapping_add(u32::from(u16::from_be_bytes([hi, lo])));
    }
    if let Some(&last) = chunks.remainder().first() {
        sum = sum.wrapping_add(u32::from(last) << 8);
    }
    !fold32(sum)
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 802.1Q single-tag frame is stripped and the inner `ether_type` is
    /// what the IPv4/IPv6 branch sees.
    #[test]
    fn vlan_tag_stripped_before_ipv4_parse() {
        // Frame: dst mac (6), src mac (6), 0x8100 (2), TCI=0x0064 (VID 100) (2),
        // inner ether_type=0x0800 (2).
        let frame: [u8; 18] = [
            0x02, 0x00, 0x00, 0x00, 0x00, 0x01, // dst
            0x02, 0x00, 0x00, 0x00, 0x00, 0x02, // src
            0x81, 0x00, // 802.1Q
            0x00, 0x64, // TCI: VID 100
            0x08, 0x00, // inner type: IPv4
        ];
        let stripped = strip_vlan(&frame).expect("valid 802.1Q frame must parse");
        assert_eq!(stripped.ether_type, 0x0800);
        assert_eq!(stripped.vlan_id, Some(100));
        assert_eq!(stripped.l3_offset, 18);
    }

    /// Untagged frame is passed through with `l3_offset == 14`.
    #[test]
    fn untagged_frame_has_14_byte_l3_offset() {
        let frame: [u8; 14] = [
            0x02, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x00, 0x00, 0x00, 0x00, 0x02, 0x08, 0x00,
        ];
        let stripped = strip_vlan(&frame).expect("valid untagged Ethernet parses");
        assert_eq!(stripped.ether_type, 0x0800);
        assert_eq!(stripped.vlan_id, None);
        assert_eq!(stripped.l3_offset, 14);
    }

    /// Truncated 802.1Q frame (missing inner type) returns None rather
    /// than panicking.
    #[test]
    fn truncated_vlan_frame_returns_none() {
        let frame: [u8; 15] = [
            0x02, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x00, 0x00, 0x00, 0x00, 0x02, 0x81, 0x00,
            0x00,
        ];
        assert!(strip_vlan(&frame).is_none());
    }

    /// IPv6 conntrack hit maps to an `XDP_TX` action with the stored
    /// backend index; misses pass through.
    #[test]
    fn ipv6_conntrack_hit_triggers_xdp_tx_sim() {
        let mut ct = ConntrackV6::new();
        let flow = FlowKeyV6 {
            src: Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1),
            dst: Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 2),
            src_port: 49_152_u16.to_be(),
            dst_port: 443_u16.to_be(),
            protocol: 6,
        };
        ct.insert(&flow, 7);
        assert_eq!(ct.decide(&flow), SimAction::Tx { backend_idx: 7 });

        let other = FlowKeyV6 {
            dst_port: 80_u16.to_be(),
            ..flow
        };
        assert_eq!(ct.decide(&other), SimAction::Pass);
    }

    /// LPM trie picks the most-specific matching prefix.
    #[test]
    fn lpm_trie_acl_matches_prefix() {
        let mut acl = AclTrie::new();
        acl.deny(Ipv4Addr::new(10, 0, 0, 0), 8); // coarse
        acl.deny(Ipv4Addr::new(10, 1, 2, 0), 24); // specific

        assert_eq!(acl.longest_match(Ipv4Addr::new(10, 1, 2, 5)), Some(24));
        assert_eq!(acl.longest_match(Ipv4Addr::new(10, 9, 9, 9)), Some(8));
        assert!(!acl.denies(Ipv4Addr::new(192, 168, 0, 1)));

        // /32 deny only matches exact addr.
        acl.deny(Ipv4Addr::new(203, 0, 113, 7), 32);
        assert_eq!(acl.longest_match(Ipv4Addr::new(203, 0, 113, 7)), Some(32));
        assert!(!acl.denies(Ipv4Addr::new(203, 0, 113, 8)));
    }

    /// /0 rule matches every address.
    #[test]
    fn lpm_trie_zero_prefix_matches_everything() {
        let mut acl = AclTrie::new();
        acl.deny(Ipv4Addr::UNSPECIFIED, 0);
        assert!(acl.denies(Ipv4Addr::new(1, 2, 3, 4)));
        assert!(acl.denies(Ipv4Addr::new(255, 255, 255, 255)));
    }

    /// Property test: for a few thousand random 16-bit field mutations,
    /// RFC 1624 incremental checksum update always agrees with the
    /// full-recompute path.
    #[test]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "low-byte / low-word of an xorshift state is exactly the random value we want"
    )]
    fn rfc_1624_incremental_checksum_matches_recompute() {
        // xorshift64* — deterministic, reproducible, no rand dep.
        fn xorshift(state: &mut u64) -> u64 {
            *state ^= *state << 13;
            *state ^= *state >> 7;
            *state ^= *state << 17;
            *state
        }

        let seeds: [u64; 4] = [
            0x5eed_5eed_5eed_5eed,
            0xdead_beef_cafe_babe,
            0x0123_4567_89ab_cdef,
            0xffff_0000_5a5a_a5a5,
        ];

        for seed in seeds {
            let mut state = seed;
            for _ in 0..2_500 {
                let mut buf = [0u8; 40];
                for b in &mut buf {
                    *b = xorshift(&mut state) as u8;
                }
                let initial_csum = full_checksum(&buf);

                // 19 aligned 16-bit lanes at offsets 0..38.
                let lane = (xorshift(&mut state) as usize) % 19;
                let offset = lane * 2;
                let old_word = u16::from_be_bytes([buf[offset], buf[offset + 1]]);
                let new_word = xorshift(&mut state) as u16;

                // Full recompute path.
                buf[offset] = (new_word >> 8) as u8;
                buf[offset + 1] = new_word as u8;
                let recomputed = full_checksum(&buf);

                // Incremental path.
                let incremental = csum16_update(initial_csum, old_word, new_word);

                assert_eq!(
                    incremental, recomputed,
                    "RFC 1624 incremental update diverged at lane {lane}: \
                     initial=0x{initial_csum:04x}, old=0x{old_word:04x}, \
                     new=0x{new_word:04x}, recomputed=0x{recomputed:04x}, \
                     incremental=0x{incremental:04x}",
                );
            }
        }
    }

    /// Incremental update is reversible: applying old→new then new→old
    /// returns the original checksum.
    #[test]
    fn rfc_1624_incremental_reversible() {
        let csum: u16 = 0x1234;
        let old = 0xABCD;
        let new = 0x5678;
        let forward = csum16_update(csum, old, new);
        let backward = csum16_update(forward, new, old);
        assert_eq!(backward, csum);
    }
}
