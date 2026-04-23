//! L4 XDP data plane for TCP/UDP load balancing.
//!
//! Parses Ethernet → IPv4 → TCP/UDP headers with verifier-safe bounds
//! checks, then decides the packet's fate:
//!
//! 1. Header bounds-check fails → `XDP_PASS` (let the kernel handle it).
//! 2. Destination port is registered in `L7_PORTS` → `XDP_PASS` so the
//!    userspace L7 stack (io_uring) receives it.
//! 3. Source /32 matches a deny entry in `ACL_DENY` → `XDP_DROP`.
//! 4. 5-tuple is pinned in `CONNTRACK` → `XDP_PASS` (full XDP_TX with
//!    checksum rewrite is deferred to Pillar 4b).
//! 5. Default → `XDP_PASS`.
//!
//! IPv6, VLAN, ICMP, and the full XDP_TX rewrite path are intentionally out
//! of scope for Pillar 4a — see ADR-0004 for the Pillar 4b plan.

#![no_std]
#![no_main]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable
)]
#![warn(clippy::pedantic)]

use core::mem;

use aya_ebpf::{
    bindings::xdp_action,
    macros::{map, xdp},
    maps::{HashMap, PerCpuArray},
    programs::XdpContext,
};

// ---------------------------------------------------------------------------
// On-wire header shapes. Repr(C) + packed(2) matches kernel layout exactly
// so field offsets are stable regardless of rustc.
// ---------------------------------------------------------------------------

const ETH_P_IP: u16 = 0x0800;
const IPPROTO_TCP: u8 = 6;
const IPPROTO_UDP: u8 = 17;

const ETH_HDR_LEN: usize = 14;
// Anchor constants for reviewers; the code enforces IHL >= 5 directly,
// so `IPV4_MIN_HDR_LEN` would be unused at codegen time.
#[allow(dead_code)]
const IPV4_MIN_HDR_LEN: usize = 20;
const TCP_MIN_HDR_LEN: usize = 20;
const UDP_HDR_LEN: usize = 8;

#[repr(C, packed(2))]
struct EthHdr {
    dst: [u8; 6],
    src: [u8; 6],
    ether_type: u16,
}

#[repr(C, packed(2))]
struct Ipv4Hdr {
    /// `version << 4 | ihl` — IHL is in 32-bit words, so real header
    /// length in bytes = `(ihl & 0x0F) * 4`.
    version_ihl: u8,
    tos: u8,
    tot_len: u16,
    id: u16,
    frag_off: u16,
    ttl: u8,
    protocol: u8,
    check: u16,
    src: u32,
    dst: u32,
}

#[repr(C, packed(2))]
struct TcpHdr {
    src_port: u16,
    dst_port: u16,
    // Remaining TCP header fields are not required for the Pillar 4a decision.
}

#[repr(C, packed(2))]
struct UdpHdr {
    src_port: u16,
    dst_port: u16,
    len: u16,
    check: u16,
}

// ---------------------------------------------------------------------------
// BPF map schemas — kept in lock-step with ADR-0005.
// ---------------------------------------------------------------------------

/// Flow 5-tuple (IPv4 only for Pillar 4a). All fields stored in network byte
/// order for zero-cost comparison against packet bytes.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FlowKey {
    pub src_addr: u32,
    pub dst_addr: u32,
    pub src_port: u16,
    pub dst_port: u16,
    pub protocol: u8,
    pub _pad: [u8; 3],
}

/// Conntrack value: where a pinned flow is steered. `backend_idx` references
/// the per-service Maglev table maintained by userspace.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BackendEntry {
    pub backend_idx: u32,
    pub flags: u32,
}

/// Stats slot indices for the `STATS` per-CPU array. Kept small because the
/// verifier limits per-CPU array sizes.
const STAT_PASS: u32 = 0;
const STAT_DROP: u32 = 1;
const STAT_CT_HIT: u32 = 2;
const STAT_L7: u32 = 3;
const STAT_PARSE_FAIL: u32 = 4;

#[map]
static CONNTRACK: HashMap<FlowKey, BackendEntry> = HashMap::<FlowKey, BackendEntry>::with_max_entries(1_000_000, 0);

#[map]
static L7_PORTS: HashMap<u16, u8> = HashMap::<u16, u8>::with_max_entries(256, 0);

// LPM_TRIE ergonomics in aya-ebpf 0.1 require &[u8; N] keys whose first byte
// is the prefix length. Pillar 4a uses a plain u32 /32 deny map instead:
// simpler, matches what the userspace ACL in lb-security pushes down today,
// and avoids a verifier quirk where LpmTrie keys on some kernels. Pillar 4b
// upgrades this to a true LPM trie (ADR-0004 follow-ups).
#[map]
static ACL_DENY: HashMap<u32, u32> = HashMap::<u32, u32>::with_max_entries(100_000, 0);

#[map]
static STATS: PerCpuArray<u64> = PerCpuArray::<u64>::with_max_entries(32, 0);

// ---------------------------------------------------------------------------
// Bounds-checked packet accessor. Verifier-safe: ptr + offset is validated
// against ctx.data_end() on every call.
// ---------------------------------------------------------------------------

/// Read a `T` at `offset` inside the packet buffer.
///
/// # Safety
///
/// The caller must ensure `T` is `Pod`-like (`#[repr(C)]`, no padding that
/// matters, no `Drop`). The function itself checks that the bytes
/// `[offset .. offset + size_of::<T>())` lie inside the XDP buffer; if not,
/// it returns `None`. This is the invariant the BPF verifier requires.
#[inline(always)]
unsafe fn ptr_at<T>(ctx: &XdpContext, offset: usize) -> Option<*const T> {
    let start = ctx.data();
    let end = ctx.data_end();
    let len = mem::size_of::<T>();
    if start + offset + len > end {
        return None;
    }
    // SAFETY: bounds just checked; pointer is within [start, end).
    Some((start + offset) as *const T)
}

/// Increment a per-CPU stats counter. Misses silently drop the increment —
/// stats are advisory, never load-bearing.
#[inline(always)]
fn incr_stat(idx: u32) {
    if let Some(slot) = STATS.get_ptr_mut(idx) {
        // SAFETY: aya returned a non-null pointer for this CPU's slot.
        unsafe {
            *slot = (*slot).wrapping_add(1);
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point.
// ---------------------------------------------------------------------------

#[xdp]
pub fn lb_xdp(ctx: XdpContext) -> u32 {
    match try_lb_xdp(&ctx) {
        Ok(action) => action,
        // Any parse failure → pass upstream; do not black-hole traffic.
        Err(()) => {
            incr_stat(STAT_PARSE_FAIL);
            xdp_action::XDP_PASS
        }
    }
}

fn try_lb_xdp(ctx: &XdpContext) -> Result<u32, ()> {
    // --- Ethernet --------------------------------------------------------
    // SAFETY: ptr_at validates bounds.
    let eth = unsafe { ptr_at::<EthHdr>(ctx, 0).ok_or(())? };
    // SAFETY: eth is validated above; field read via read_unaligned for
    // #[repr(packed)] correctness.
    let ether_type = u16::from_be(unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*eth).ether_type)) });
    if ether_type != ETH_P_IP {
        // IPv6 and VLAN deferred to Pillar 4b.
        incr_stat(STAT_PASS);
        return Ok(xdp_action::XDP_PASS);
    }

    // --- IPv4 ------------------------------------------------------------
    // SAFETY: ptr_at validates bounds for a minimum-sized IPv4 header.
    let ip = unsafe { ptr_at::<Ipv4Hdr>(ctx, ETH_HDR_LEN).ok_or(())? };
    // SAFETY: ip is validated; #[repr(packed)] requires read_unaligned.
    let version_ihl = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*ip).version_ihl)) };
    let ihl_words = (version_ihl & 0x0F) as usize;
    // IHL must be at least 5 (20 bytes). Reject malformed / options-heavy
    // packets beyond the verifier-friendly maximum of 15 words (60 bytes).
    if ihl_words < 5 {
        return Err(());
    }
    let ip_hdr_len = ihl_words * 4;
    // SAFETY: packed field, read_unaligned required.
    let protocol = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*ip).protocol)) };
    // SAFETY: packed field reads.
    let src_addr = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*ip).src)) };
    // SAFETY: packed field reads.
    let dst_addr = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*ip).dst)) };

    // --- ACL check (pre-conntrack, denies wire off first-hop) ------------
    // ACL keys in ACL_DENY are stored in the same byte order as arrives on
    // the wire; no htonl/ntohl dance.
    if unsafe { ACL_DENY.get(&src_addr) }.is_some() {
        incr_stat(STAT_DROP);
        return Ok(xdp_action::XDP_DROP);
    }

    // --- L4 transport layer ----------------------------------------------
    let l4_offset = ETH_HDR_LEN + ip_hdr_len;
    let (src_port, dst_port) = match protocol {
        IPPROTO_TCP => {
            // SAFETY: ptr_at validates bounds.
            let tcp = unsafe { ptr_at::<TcpHdr>(ctx, l4_offset).ok_or(())? };
            // Pillar 4b will verify TCP header length (data offset) for
            // options handling. For the 4a decision the min header suffices.
            let _ = TCP_MIN_HDR_LEN; // Keep the constant anchored against drift.
            // SAFETY: packed reads.
            let sp = u16::from_be(unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*tcp).src_port)) });
            let dp = u16::from_be(unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*tcp).dst_port)) });
            (sp, dp)
        }
        IPPROTO_UDP => {
            // SAFETY: ptr_at validates bounds.
            let udp = unsafe { ptr_at::<UdpHdr>(ctx, l4_offset).ok_or(())? };
            let _ = UDP_HDR_LEN; // Anchor constant.
            // SAFETY: packed reads.
            let sp = u16::from_be(unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*udp).src_port)) });
            let dp = u16::from_be(unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*udp).dst_port)) });
            (sp, dp)
        }
        _ => {
            // ICMP, SCTP, … deferred. Pass upstream.
            incr_stat(STAT_PASS);
            return Ok(xdp_action::XDP_PASS);
        }
    };

    // --- L7 bypass: userspace owns this port -----------------------------
    if unsafe { L7_PORTS.get(&dst_port) }.is_some() {
        incr_stat(STAT_L7);
        return Ok(xdp_action::XDP_PASS);
    }

    // --- Conntrack pinning ----------------------------------------------
    // The only moving pieces between packets on the same flow are the
    // ports; addrs and protocol are stable. We build the key in the same
    // byte order BPF sees on the wire.
    let key = FlowKey {
        src_addr,
        dst_addr,
        src_port: src_port.to_be(),
        dst_port: dst_port.to_be(),
        protocol,
        _pad: [0; 3],
    };
    if unsafe { CONNTRACK.get(&key) }.is_some() {
        incr_stat(STAT_CT_HIT);
        // Pillar 4b will look up the BackendEntry, rewrite the packet, and
        // return XDP_TX. For 4a we pass through so the userspace fast path
        // keeps handling it.
        return Ok(xdp_action::XDP_PASS);
    }

    incr_stat(STAT_PASS);
    Ok(xdp_action::XDP_PASS)
}

// Required by the BPF linker: a panic handler for `no_std` + `panic=abort`.
#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    // The verifier forbids loops; spin via an unreachable hint.
    loop {}
}
