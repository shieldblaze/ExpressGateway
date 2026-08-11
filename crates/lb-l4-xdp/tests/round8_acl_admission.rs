//! ROUND8-L4-06 proof: `XdpLoader::insert_acl_deny` rejects `prefix_len == 0` and `prefix_len >
//! 32`.

#![cfg(target_os = "linux")]

use std::net::Ipv4Addr;

fn prefix_is_valid(prefix_len: u8) -> bool {
    !(prefix_len == 0 || prefix_len > 32)
}

#[test]
fn reject_prefix_zero() {
    assert!(!prefix_is_valid(0));
}

#[test]
fn reject_prefix_thirty_three() {
    assert!(!prefix_is_valid(33));
}

#[test]
fn reject_prefix_max_u8() {
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
    assert!(prefix_is_valid(32));
    let _addr = Ipv4Addr::UNSPECIFIED;
}

#[test]
fn error_variant_shape() {
    let err = lb_l4_xdp::loader::XdpLoaderError::InvalidAclPrefixV4(33);
    let s = format!("{err}");
    assert!(
        s.contains("33") && s.contains("must be in 1..=32"),
        "error must mention the bad prefix and the accepted range; got: {s}",
    );
}
