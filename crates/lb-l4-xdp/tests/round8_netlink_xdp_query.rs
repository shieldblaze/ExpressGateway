//! ROUND8-L4-12 proof: real RTM_GETLINK XDP prog-id byte parser.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use lb_l4_xdp::netlink_xdp::{XdpLinkInfo, parse_getlink_response, parse_ifinfo_payload};

const RTM_NEWLINK: u16 = 16;
const NLMSG_DONE: u16 = 0x3;
const NLMSG_ERROR: u16 = 0x2;
const IFLA_XDP: u16 = 43;
const IFLA_XDP_PROG_ID: u16 = 4;
const IFLA_XDP_ATTACHED: u16 = 2;
const IFLA_IFNAME: u16 = 3;

fn align4(n: usize) -> usize {
    (n + 3) & !3
}

/// Emit one `rtattr` (header + payload + 4-byte padding) into `out`.
fn push_rtattr(out: &mut Vec<u8>, atype: u16, payload: &[u8]) {
    let rta_len = 4 + payload.len();
    out.extend_from_slice(&(rta_len as u16).to_ne_bytes());
    out.extend_from_slice(&atype.to_ne_bytes());
    out.extend_from_slice(payload);
    while out.len() % 4 != 0 {
        out.push(0);
    }
    let _ = align4(rta_len);
}

/// Build a full `RTM_NEWLINK` netlink datagram for an interface that has an XDP program (prog_id `prog`) attached in mode `mode_byte`, terminated by an `NLMSG_DONE`.
fn build_link_reply_with_xdp(ifindex: i32, ifname: &str, prog: u32, mode_byte: u8) -> Vec<u8> {
    let mut ifi = Vec::new();
    ifi.push(0u8); // AF_UNSPEC
    ifi.push(0u8); // pad
    ifi.extend_from_slice(&1u16.to_ne_bytes()); // ARPHRD_ETHER
    ifi.extend_from_slice(&ifindex.to_ne_bytes());
    ifi.extend_from_slice(&0u32.to_ne_bytes()); // flags
    ifi.extend_from_slice(&0u32.to_ne_bytes()); // change

    let mut attrs = Vec::new();
    let mut name_buf = ifname.as_bytes().to_vec();
    name_buf.push(0); // NUL-terminated
    push_rtattr(&mut attrs, IFLA_IFNAME, &name_buf);

    let mut xdp_nested = Vec::new();
    push_rtattr(&mut xdp_nested, IFLA_XDP_PROG_ID, &prog.to_ne_bytes());
    push_rtattr(&mut xdp_nested, IFLA_XDP_ATTACHED, &[mode_byte]);
    push_rtattr(&mut attrs, IFLA_XDP, &xdp_nested);

    let body_len = ifi.len() + attrs.len();
    let nlmsg_len = 16 + body_len;

    let mut msg = Vec::new();
    msg.extend_from_slice(&(nlmsg_len as u32).to_ne_bytes());
    msg.extend_from_slice(&RTM_NEWLINK.to_ne_bytes());
    msg.extend_from_slice(&0u16.to_ne_bytes()); // flags
    msg.extend_from_slice(&1u32.to_ne_bytes()); // seq
    msg.extend_from_slice(&4321u32.to_ne_bytes()); // pid
    msg.extend_from_slice(&ifi);
    msg.extend_from_slice(&attrs);
    while msg.len() % 4 != 0 {
        msg.push(0);
    }

    let mut done = Vec::new();
    done.extend_from_slice(&16u32.to_ne_bytes());
    done.extend_from_slice(&NLMSG_DONE.to_ne_bytes());
    done.extend_from_slice(&0u16.to_ne_bytes());
    done.extend_from_slice(&1u32.to_ne_bytes());
    done.extend_from_slice(&4321u32.to_ne_bytes());

    msg.extend_from_slice(&done);
    msg
}

/// Build the same shape but with NO `IFLA_XDP` attribute — the "interface has no XDP program" / post-detach success case.
fn build_link_reply_no_xdp(ifindex: i32, ifname: &str) -> Vec<u8> {
    let mut ifi = Vec::new();
    ifi.push(0u8);
    ifi.push(0u8);
    ifi.extend_from_slice(&1u16.to_ne_bytes());
    ifi.extend_from_slice(&ifindex.to_ne_bytes());
    ifi.extend_from_slice(&0u32.to_ne_bytes());
    ifi.extend_from_slice(&0u32.to_ne_bytes());

    let mut attrs = Vec::new();
    let mut name_buf = ifname.as_bytes().to_vec();
    name_buf.push(0);
    push_rtattr(&mut attrs, IFLA_IFNAME, &name_buf);

    let nlmsg_len = 16 + ifi.len() + attrs.len();
    let mut msg = Vec::new();
    msg.extend_from_slice(&(nlmsg_len as u32).to_ne_bytes());
    msg.extend_from_slice(&RTM_NEWLINK.to_ne_bytes());
    msg.extend_from_slice(&0u16.to_ne_bytes());
    msg.extend_from_slice(&1u32.to_ne_bytes());
    msg.extend_from_slice(&4321u32.to_ne_bytes());
    msg.extend_from_slice(&ifi);
    msg.extend_from_slice(&attrs);
    while msg.len() % 4 != 0 {
        msg.push(0);
    }
    msg
}

#[test]
fn extracts_prog_id_from_real_shaped_getlink_reply() {
    let blob = build_link_reply_with_xdp(2, "eth0", 0xDEAD_BEEF, 1);
    let info = parse_getlink_response(&blob).expect("well-formed RTM_NEWLINK must parse");
    assert_eq!(
        info.prog_id,
        Some(0xDEAD_BEEF),
        "must extract the kernel bpf_prog_info.id from IFLA_XDP_PROG_ID"
    );
    assert_eq!(info.attached_mode, Some(1));
}

#[test]
fn no_xdp_attribute_means_prog_id_none() {
    let blob = build_link_reply_no_xdp(2, "eth0");
    let info = parse_getlink_response(&blob).expect("no-XDP reply must still parse");
    assert_eq!(info.prog_id, None);
    assert_eq!(info, XdpLinkInfo::default());
}

#[test]
fn prog_id_zero_is_normalised_to_none() {
    let blob = build_link_reply_with_xdp(2, "eth0", 0, 0);
    let info = parse_getlink_response(&blob).unwrap();
    assert_eq!(info.prog_id, None);
}

#[test]
fn nlmsg_error_with_errno_is_surfaced() {
    let mut msg = Vec::new();
    let body_errno: i32 = -1; // -EPERM
    let nlmsg_len = 16 + 4;
    msg.extend_from_slice(&(nlmsg_len as u32).to_ne_bytes());
    msg.extend_from_slice(&NLMSG_ERROR.to_ne_bytes());
    msg.extend_from_slice(&0u16.to_ne_bytes());
    msg.extend_from_slice(&1u32.to_ne_bytes());
    msg.extend_from_slice(&4321u32.to_ne_bytes());
    msg.extend_from_slice(&body_errno.to_ne_bytes());

    let err = parse_getlink_response(&msg).expect_err("NLMSG_ERROR with errno must be an Err");
    assert_eq!(err.raw_os_error(), Some(1), "must surface EPERM(1)");
}

#[test]
fn truncated_message_is_rejected_not_panicked() {
    let mut msg = Vec::new();
    msg.extend_from_slice(&9999u32.to_ne_bytes()); // lying length
    msg.extend_from_slice(&RTM_NEWLINK.to_ne_bytes());
    msg.extend_from_slice(&0u16.to_ne_bytes());
    msg.extend_from_slice(&1u32.to_ne_bytes());
    msg.extend_from_slice(&4321u32.to_ne_bytes());
    assert!(parse_getlink_response(&msg).is_err());
}

#[test]
fn malformed_nested_rtattr_does_not_loop_or_panic() {
    let mut ifi = vec![0u8; 16];
    ifi[8] = 2; // ifi_index low byte
    let mut bad_nested = Vec::new();
    bad_nested.extend_from_slice(&0u16.to_ne_bytes()); // rta_len = 0 (bad)
    bad_nested.extend_from_slice(&IFLA_XDP_PROG_ID.to_ne_bytes());
    let mut attrs = Vec::new();
    push_rtattr(&mut attrs, IFLA_XDP, &bad_nested);
    let mut payload = ifi;
    payload.extend_from_slice(&attrs);
    let info = parse_ifinfo_payload(&payload);
    assert_eq!(info.prog_id, None);
}

#[test]
fn empty_buffer_is_default_not_error() {
    let info = parse_getlink_response(&[]).unwrap();
    assert_eq!(info, XdpLinkInfo::default());
}

/// Privileged CI lane: exercise the live AF_NETLINK socket path against the loopback interface (always present).
#[test]
#[ignore = "live AF_NETLINK socket — privileged CI lane only"]
fn live_query_loopback_has_no_xdp() {
    let r =
        lb_l4_xdp::netlink_xdp::query_xdp_prog_id("lo").expect("RTM_GETLINK on lo must succeed");
    assert_eq!(r, None, "loopback has no XDP program in CI");
}
