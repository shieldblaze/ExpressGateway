//! ROUND8-L4-12 proof: real RTM_GETLINK XDP prog-id byte parser.
//!
//! Reference: kernel UAPI `linux/if_link.h` (`IFLA_XDP` /
//! `IFLA_XDP_PROG_ID`, stable since Linux 4.13) and `iproute2`
//! `ip/iplink_xdp.c` / `bpftool net`'s `RTM_GETLINK` decode path.
//!
//! The verifier (audit/round-8/verify/l4.md L4-12) accepted the API
//! scaffold with caveat: `query_xdp` was a `prog_id: None` stub, so
//! the EBUSY-on-redeploy bug (finding ROUND8-L4-12 failure mode A)
//! was functionally OPEN — `detach_verifying`/`attach_replacing`
//! could not see real kernel state. This test is the floor proof
//! that the replacement byte parser extracts the kernel `prog_id`
//! from a *real-shaped* netlink response without needing
//! CAP_NET_ADMIN or a live kernel (the live socket path is exercised
//! on the privileged CI lane).
//!
//! The fixtures below are hand-assembled to the exact kernel wire
//! layout (verified against `linux/rtnetlink.h` /
//! `linux/netlink.h`): a 16-byte `nlmsghdr`, a 16-byte `ifinfomsg`,
//! then NLMSG-aligned `rtattr` TLVs, with `IFLA_XDP` (43) as a
//! NESTED container holding `IFLA_XDP_PROG_ID` (4, u32) and
//! `IFLA_XDP_ATTACHED` (2, u8).

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

/// Build a full `RTM_NEWLINK` netlink datagram for an interface that
/// has an XDP program (prog_id `prog`) attached in mode `mode_byte`,
/// terminated by an `NLMSG_DONE`.
fn build_link_reply_with_xdp(ifindex: i32, ifname: &str, prog: u32, mode_byte: u8) -> Vec<u8> {
    // ifinfomsg: family(1) pad(1) type(2) index(4) flags(4) change(4)
    let mut ifi = Vec::new();
    ifi.push(0u8); // AF_UNSPEC
    ifi.push(0u8); // pad
    ifi.extend_from_slice(&1u16.to_ne_bytes()); // ARPHRD_ETHER
    ifi.extend_from_slice(&ifindex.to_ne_bytes());
    ifi.extend_from_slice(&0u32.to_ne_bytes()); // flags
    ifi.extend_from_slice(&0u32.to_ne_bytes()); // change

    // Attributes: an unrelated IFLA_IFNAME first (proves the parser
    // skips siblings), then the nested IFLA_XDP container.
    let mut attrs = Vec::new();
    let mut name_buf = ifname.as_bytes().to_vec();
    name_buf.push(0); // NUL-terminated
    push_rtattr(&mut attrs, IFLA_IFNAME, &name_buf);

    // Nested IFLA_XDP block: prog_id (u32) + attached (u8).
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

    // Trailing NLMSG_DONE.
    let mut done = Vec::new();
    done.extend_from_slice(&16u32.to_ne_bytes());
    done.extend_from_slice(&NLMSG_DONE.to_ne_bytes());
    done.extend_from_slice(&0u16.to_ne_bytes());
    done.extend_from_slice(&1u32.to_ne_bytes());
    done.extend_from_slice(&4321u32.to_ne_bytes());

    msg.extend_from_slice(&done);
    msg
}

/// Build the same shape but with NO `IFLA_XDP` attribute — the
/// "interface has no XDP program" / post-detach success case.
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
    // eth0, ifindex 2, prog_id 0xDEAD_BEEF, attached mode 1 (DRV).
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
    // This is the post-detach success signal `detach_verifying`
    // depends on: an interface with no IFLA_XDP block -> None.
    let blob = build_link_reply_no_xdp(2, "eth0");
    let info = parse_getlink_response(&blob).expect("no-XDP reply must still parse");
    assert_eq!(info.prog_id, None);
    assert_eq!(info, XdpLinkInfo::default());
}

#[test]
fn prog_id_zero_is_normalised_to_none() {
    // The kernel never hands out prog_id 0; a 0 in the attribute is
    // "nothing meaningful attached" — must NOT be reported as
    // Some(0) (which would make attach_replacing think a foreign
    // prog id 0 owns the iface).
    let blob = build_link_reply_with_xdp(2, "eth0", 0, 0);
    let info = parse_getlink_response(&blob).unwrap();
    assert_eq!(info.prog_id, None);
}

#[test]
fn nlmsg_error_with_errno_is_surfaced() {
    // An NLMSG_ERROR with a non-zero (negative) errno must become an
    // Err so the loader fails loud instead of treating a kernel
    // refusal as "no program attached".
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
    // Hostile / partial datagram: nlmsg_len claims more than the
    // buffer holds. Must be a clean Err, never a panic or OOB index.
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
    // A zero-length rtattr inside the IFLA_XDP nest must terminate
    // iteration (rta_len < 4 is malformed) without an infinite loop.
    let mut ifi = vec![0u8; 16];
    ifi[8] = 2; // ifi_index low byte
    let mut bad_nested = Vec::new();
    bad_nested.extend_from_slice(&0u16.to_ne_bytes()); // rta_len = 0 (bad)
    bad_nested.extend_from_slice(&IFLA_XDP_PROG_ID.to_ne_bytes());
    let mut attrs = Vec::new();
    push_rtattr(&mut attrs, IFLA_XDP, &bad_nested);
    let mut payload = ifi;
    payload.extend_from_slice(&attrs);
    // Direct payload parse (post-nlmsghdr) must yield None, no hang.
    let info = parse_ifinfo_payload(&payload);
    assert_eq!(info.prog_id, None);
}

#[test]
fn empty_buffer_is_default_not_error() {
    // Zero-length recv (shouldn't happen on a healthy socket) is
    // treated as "no info" rather than an error so an idempotent
    // re-query during drain stays quiet.
    let info = parse_getlink_response(&[]).unwrap();
    assert_eq!(info, XdpLinkInfo::default());
}

/// Privileged CI lane: exercise the live AF_NETLINK socket path
/// against the loopback interface (always present). Ignored in the
/// sandbox because even an unprivileged RTM_GETLINK read needs a
/// usable netlink socket which the seccomp sandbox may deny.
#[test]
#[ignore = "live AF_NETLINK socket — privileged CI lane only"]
fn live_query_loopback_has_no_xdp() {
    let r =
        lb_l4_xdp::netlink_xdp::query_xdp_prog_id("lo").expect("RTM_GETLINK on lo must succeed");
    assert_eq!(r, None, "loopback has no XDP program in CI");
}
