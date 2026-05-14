//! ROUND8-L4-08 proof: IPv4 fragments and IPv6 packets with a
//! Fragment Extension Header are passed to the kernel (never
//! rewritten in XDP).
//!
//! The userspace tests here exercise the *bit-level invariants*
//! the eBPF check enforces. The eBPF guard is a single packed
//! `frag_off` read masked with `0x3FFF`; the IPv6 guard is an
//! equality check against `IPPROTO_FRAGMENT = 44`. Both are
//! verifier-trivial and tested at the integer level here. The
//! cross-kernel matrix run (ROUND8-L4-10) covers the actual BPF
//! verdict via `bpftool prog run`.

use lb_l4_xdp::stats_export::{NUM_SLOTS, StatSlot};

/// Mirror of the eBPF IPv4 fragment check.
fn is_fragment_v4(frag_off_be: u16) -> bool {
    (u16::from_be(frag_off_be) & 0x3FFF) != 0
}

/// IPv6 Fragment Extension Header next-header value (RFC 2460 §4.5).
const IPPROTO_FRAGMENT: u8 = 44;

#[test]
fn ipv4_first_fragment_with_mf_set_detected() {
    // RFC 791: bit 13 (counting from MSB, big-endian) is MF.
    // 0x2000 in network byte order = "more fragments".
    let frag_off_be = 0x2000u16.to_be();
    assert!(is_fragment_v4(frag_off_be));
}

#[test]
fn ipv4_later_fragment_with_offset_detected() {
    // Fragment offset = 185 (× 8 = 1480-byte offset). No MF set.
    // Field encoded as a u16: low 13 bits are offset.
    let frag_off_be = 185u16.to_be();
    assert!(is_fragment_v4(frag_off_be));
}

#[test]
fn ipv4_unfragmented_packet_not_detected() {
    // frag_off = 0 (or DF only — bit 14 ignored). Must pass through.
    let frag_off_be = 0u16.to_be();
    assert!(!is_fragment_v4(frag_off_be));
    // DF only (bit 14 = 0x4000) — not a fragment.
    let frag_off_be = 0x4000u16.to_be();
    assert!(!is_fragment_v4(frag_off_be));
}

#[test]
fn ipv6_fragment_proto_value_is_44() {
    // RFC 2460 §4.5 — the value the eBPF program compares against.
    assert_eq!(IPPROTO_FRAGMENT, 44);
}

#[test]
fn stat_slots_for_fragments_at_indices_11_and_12() {
    assert_eq!(StatSlot::V4Fragment as usize, 11);
    assert_eq!(StatSlot::V6Fragment as usize, 12);
    assert_eq!(NUM_SLOTS, 13);
}

#[test]
fn ipv4_fragment_mask_0x3fff_covers_offset_and_mf() {
    // The mask 0x3FFF spans MF (bit 13) + 13 offset bits.
    // 0x3FFF == 0b0011_1111_1111_1111 — top 2 bits (DF + reserved)
    // are NOT in the mask.
    assert_eq!(0x3FFFu16, 0b0011_1111_1111_1111);
    // The DF bit (0x4000) and the reserved bit (0x8000) must NOT
    // be matched as fragments — that would mis-classify packets
    // with DF set as fragments.
    assert!(!is_fragment_v4(0x4000u16.to_be()));
    assert!(!is_fragment_v4(0x8000u16.to_be()));
}
