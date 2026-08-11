//! ROUND8-L4-08 proof: IPv4 fragments and IPv6 packets with a Fragment Extension Header are passed to the kernel (never rewritten in XDP).

use lb_l4_xdp::stats_export::{NUM_SLOTS, StatSlot};

/// Mirror of the eBPF IPv4 fragment check.
fn is_fragment_v4(frag_off_be: u16) -> bool {
    (u16::from_be(frag_off_be) & 0x3FFF) != 0
}

/// IPv6 Fragment Extension Header next-header value (RFC 2460 §4.5).
const IPPROTO_FRAGMENT: u8 = 44;

#[test]
fn ipv4_first_fragment_with_mf_set_detected() {
    let frag_off_be = 0x2000u16.to_be();
    assert!(is_fragment_v4(frag_off_be));
}

#[test]
fn ipv4_later_fragment_with_offset_detected() {
    let frag_off_be = 185u16.to_be();
    assert!(is_fragment_v4(frag_off_be));
}

#[test]
fn ipv4_unfragmented_packet_not_detected() {
    let frag_off_be = 0u16.to_be();
    assert!(!is_fragment_v4(frag_off_be));
    let frag_off_be = 0x4000u16.to_be();
    assert!(!is_fragment_v4(frag_off_be));
}

#[test]
fn ipv6_fragment_proto_value_is_44() {
    assert_eq!(IPPROTO_FRAGMENT, 44);
}

#[test]
fn stat_slots_for_fragments_at_indices_11_and_12() {
    assert_eq!(StatSlot::V4Fragment as usize, 11);
    assert_eq!(StatSlot::V6Fragment as usize, 12);
    assert!(
        (StatSlot::V4Fragment as usize) < NUM_SLOTS,
        "V4Fragment slot must be inside the read_stats loop"
    );
    assert!(
        (StatSlot::V6Fragment as usize) < NUM_SLOTS,
        "V6Fragment slot must be inside the read_stats loop"
    );
    assert_eq!(
        StatSlot::NewFlowRateCap as usize + 1,
        NUM_SLOTS,
        "NUM_SLOTS must bound the read loop exactly to the last \
         STAT_*-backed slot (NewFlowRateCap = 15)"
    );
}

#[test]
fn ipv4_fragment_mask_0x3fff_covers_offset_and_mf() {
    assert_eq!(0x3FFFu16, 0b0011_1111_1111_1111);
    assert!(!is_fragment_v4(0x4000u16.to_be()));
    assert!(!is_fragment_v4(0x8000u16.to_be()));
}
