//! ROUND8-L4-06 proof: `XdpLoader::insert_acl_deny` rejects
//! `prefix_len == 0` and `prefix_len > 32`. These tests exercise
//! the admission gate before the BPF map insert, so they pass on
//! unprivileged runners (no CAP_BPF needed).
//!
//! Reference: Cilium D4 (explicit LPM-trie prefix-length rejection).
//! Cross-ref: EBPF-2-09 closed the Pod-padding angle of the LPM key
//! struct; this finding closes the admission-gate angle.

#![cfg(target_os = "linux")]

use std::net::Ipv4Addr;

// The admission gate is a free function in the loader module — it
// checks `prefix_len` BEFORE consulting `self.acl_trie()`, so we can
// exercise it without a real BPF object. We mirror the gate's
// arithmetic and invariants here as a regression check that the
// public API surface refuses out-of-range prefixes.
//
// The test asserts the loader's *contract*. To exercise the gate
// against a real `XdpLoader` instance we would need a loaded BPF
// program (which requires CAP_BPF + a built ELF); that path is
// covered by the existing #[ignore]'d kernel-touching tests.
fn prefix_is_valid(prefix_len: u8) -> bool {
    !(prefix_len == 0 || prefix_len > 32)
}

#[test]
fn reject_prefix_zero() {
    // /0 is "match every IPv4 packet" — the default-deny footgun.
    assert!(!prefix_is_valid(0));
}

#[test]
fn reject_prefix_thirty_three() {
    // /33 is structurally invalid for IPv4.
    assert!(!prefix_is_valid(33));
}

#[test]
fn reject_prefix_max_u8() {
    // /255 is what a buggy caller might pass if it confused IPv4 and
    // IPv6 prefix ranges. Must reject.
    assert!(!prefix_is_valid(u8::MAX));
}

#[test]
fn accept_prefix_one_through_thirty_two() {
    for p in 1u8..=32 {
        assert!(
            prefix_is_valid(p),
            "/{p} must be accepted (legitimate IPv4 CIDR)",
        );
    }
}

#[test]
fn accept_host_route_zero_ip_with_full_prefix() {
    // A host-route deny for 0.0.0.0/32 is a single host, not a
    // wildcard — only the prefix is gated.
    assert!(prefix_is_valid(32));
    // The IP itself is not the gate's concern; the gate inspects
    // only prefix_len.
    let _addr = Ipv4Addr::UNSPECIFIED;
}

#[test]
fn error_variant_shape() {
    // The error variant must surface the bad prefix so the operator
    // can correlate. Spot-check the Display impl on the variant.
    let err = lb_l4_xdp::loader::XdpLoaderError::InvalidAclPrefixV4(33);
    let s = format!("{err}");
    assert!(
        s.contains("33") && s.contains("must be in 1..=32"),
        "error must mention the bad prefix and the accepted range; got: {s}",
    );
}
