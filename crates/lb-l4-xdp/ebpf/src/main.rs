//! L4 XDP data plane for TCP/UDP load balancing.
//!
//! Packet flow (Pillar 4a → 4b-1 → 4b-2):
//!
//! 1. Parse Ethernet, optionally stripping one 802.1Q VLAN tag
//!    (single-tag only — QinQ is Pillar 4b-3).
//! 2. Branch on ether_type: IPv4 (0x0800) or IPv6 (0x86DD). Anything
//!    else → `XDP_PASS`.
//! 3. IPv4 path: parse header (IHL ≥ 5), consult `ACL_DENY_TRIE`
//!    (LPM trie) on src IP, then parse TCP/UDP ports, check
//!    `L7_PORTS`, then `CONNTRACK`. On a hit, rewrite MAC + dst IP +
//!    dst port with incremental RFC 1624 checksum updates and return
//!    `XDP_TX`. On a miss, `XDP_PASS` so userspace picks a backend.
//! 4. IPv6 path: parse 40-byte header, skip up to two extension
//!    headers (Hop-by-Hop, Routing — others → `XDP_PASS`), then
//!    TCP/UDP ports, then `CONNTRACK_V6`. On a hit, rewrite MAC + dst
//!    IPv6 + dst port. IPv6 has no L3 checksum (no incremental L3
//!    update needed) but the L4 checksum covers the pseudo-header, so
//!    RFC 1624 still applies.
//! 5. On any bounds-check failure → `XDP_PASS` (let the kernel handle
//!    it). Never `XDP_DROP` on parse failure.
//!
//! Deferred to Pillar 4b-3: SYN-cookie style `XDP_TX` for new flows,
//! QinQ, TCP option rewrite, `xtask xdp-verify` multi-kernel matrix.

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
    maps::{HashMap, LpmTrie, PerCpuArray, lpm_trie::Key as LpmKey},
    programs::XdpContext,
};

// ---------------------------------------------------------------------------
// Wire constants and header shapes. Repr(C, packed(2)) pins kernel layout.
// ---------------------------------------------------------------------------

const ETH_P_IP: u16 = 0x0800;
const ETH_P_IPV6: u16 = 0x86DD;
const ETH_P_8021Q: u16 = 0x8100;
const IPPROTO_TCP: u8 = 6;
const IPPROTO_UDP: u8 = 17;
const IPPROTO_HOPOPTS: u8 = 0;
const IPPROTO_ROUTING: u8 = 43;

const ETH_HDR_LEN: usize = 14;
const VLAN_HDR_LEN: usize = 4;
const IPV4_MIN_HDR_LEN: usize = 20;
const IPV6_HDR_LEN: usize = 40;
const TCP_MIN_HDR_LEN: usize = 20;
const UDP_HDR_LEN: usize = 8;
const _: () = {
    // Anchor constants so the file survives later refactors.
    let _ = IPV4_MIN_HDR_LEN;
    let _ = TCP_MIN_HDR_LEN;
    let _ = UDP_HDR_LEN;
};

#[repr(C, packed(2))]
struct EthHdr {
    dst: [u8; 6],
    src: [u8; 6],
    ether_type: u16,
}

#[repr(C, packed(2))]
struct VlanHdr {
    /// Priority (3) | DEI (1) | VLAN id (12). Unused by Pillar 4b-2; the
    /// parser only cares about `ether_type` after the tag.
    _tci: u16,
    ether_type: u16,
}

#[repr(C, packed(2))]
struct Ipv4Hdr {
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
struct Ipv6Hdr {
    /// version (4) | traffic_class (8) | flow_label (20).
    _ver_tc_fl: u32,
    payload_len: u16,
    next_header: u8,
    hop_limit: u8,
    src: [u8; 16],
    dst: [u8; 16],
}

/// IPv6 extension-header shape; only `next_header` + `hdr_ext_len` are
/// actually consulted. `hdr_ext_len` is in 8-byte units, not counting the
/// first 8 bytes — same convention as the kernel.
#[repr(C, packed(2))]
struct Ipv6ExtHdr {
    next_header: u8,
    hdr_ext_len: u8,
}

#[repr(C, packed(2))]
struct TcpHdr {
    src_port: u16,
    dst_port: u16,
    // Pillar 4b-2 does not parse further TCP fields.
}

#[repr(C, packed(2))]
struct UdpHdr {
    src_port: u16,
    dst_port: u16,
    len: u16,
    check: u16,
}

// ---------------------------------------------------------------------------
// Map schemas — aligned with ADR-0005 (Pillar 4b-2 revision).
// ---------------------------------------------------------------------------

/// IPv4 flow 5-tuple. All fields stored in network byte order.
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

/// IPv6 flow 5-tuple. Padded to a natural size the verifier likes
/// (16 + 16 + 2 + 2 + 1 + 3 = 40 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FlowKeyV6 {
    pub src_addr: [u8; 16],
    pub dst_addr: [u8; 16],
    pub src_port: u16,
    pub dst_port: u16,
    pub protocol: u8,
    pub _pad: [u8; 3],
}

/// Conntrack value for IPv4 flows. Carries the full rewrite state so
/// the BPF program needs no secondary lookup to run an `XDP_TX`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BackendEntry {
    pub backend_idx: u32,
    pub flags: u32,
    /// Backend IPv4 addr (network byte order).
    pub backend_ip: u32,
    /// Backend L4 port (network byte order).
    pub backend_port: u16,
    pub _pad: u16,
    /// Backend Ethernet MAC (destination).
    pub backend_mac: [u8; 6],
    /// Our interface's source MAC for the rewrite.
    pub src_mac: [u8; 6],
}

/// Conntrack value for IPv6 flows.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BackendEntryV6 {
    pub backend_idx: u32,
    pub flags: u32,
    pub backend_ip: [u8; 16],
    pub backend_port: u16,
    pub _pad: u16,
    pub backend_mac: [u8; 6],
    pub src_mac: [u8; 6],
}

/// Stats slots.
const STAT_PASS: u32 = 0;
const STAT_DROP: u32 = 1;
const STAT_CT_HIT_V4: u32 = 2;
const STAT_L7: u32 = 3;
const STAT_PARSE_FAIL: u32 = 4;
const STAT_TX_V4: u32 = 5;
const STAT_CT_HIT_V6: u32 = 6;
const STAT_TX_V6: u32 = 7;
const STAT_VLAN: u32 = 8;
const STAT_V6_EXT_UNSUPPORTED: u32 = 9;

#[map]
static CONNTRACK: HashMap<FlowKey, BackendEntry> =
    HashMap::<FlowKey, BackendEntry>::with_max_entries(1_000_000, 0);

#[map]
static CONNTRACK_V6: HashMap<FlowKeyV6, BackendEntryV6> =
    HashMap::<FlowKeyV6, BackendEntryV6>::with_max_entries(512_000, 0);

#[map]
static L7_PORTS: HashMap<u16, u8> = HashMap::<u16, u8>::with_max_entries(256, 0);

/// IPv4 deny ACL as a longest-prefix-match trie. Key data is the IPv4
/// address in network byte order; `prefix_len` is the CIDR mask length.
/// Pillar 4b-2 upgrade from the Pillar 4a HashMap<u32,u32>.
#[map]
static ACL_DENY_TRIE: LpmTrie<u32, u32> = LpmTrie::<u32, u32>::with_max_entries(100_000, 0);

#[map]
static STATS: PerCpuArray<u64> = PerCpuArray::<u64>::with_max_entries(32, 0);

// ---------------------------------------------------------------------------
// Verifier-safe packet accessors.
// ---------------------------------------------------------------------------

#[inline(always)]
unsafe fn ptr_at<T>(ctx: &XdpContext, offset: usize) -> Option<*const T> {
    let start = ctx.data();
    let end = ctx.data_end();
    let len = mem::size_of::<T>();
    if start + offset + len > end {
        return None;
    }
    // SAFETY: bounds validated; pointer is within [start, end).
    Some((start + offset) as *const T)
}

#[inline(always)]
unsafe fn ptr_at_mut<T>(ctx: &XdpContext, offset: usize) -> Option<*mut T> {
    let start = ctx.data();
    let end = ctx.data_end();
    let len = mem::size_of::<T>();
    if start + offset + len > end {
        return None;
    }
    // SAFETY: bounds validated.
    Some((start + offset) as *mut T)
}

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
// RFC 1624 incremental checksum helpers.
//
// RFC 1624 §3 formula:
//     HC' = ~(~HC + ~m + m')
// where HC is the old checksum (already ones-complement-encoded), m is
// the old 16-bit field, m' is the new 16-bit field, and HC' is the new
// checksum. This operates on already-folded ones-complement sums; fold
// carries at the end.
// ---------------------------------------------------------------------------

/// Fold a 32-bit one's-complement sum to 16 bits.
#[inline(always)]
fn fold32(mut sum: u32) -> u16 {
    // Two folds suffice for any u32 input.
    sum = (sum & 0xFFFF) + (sum >> 16);
    sum = (sum & 0xFFFF) + (sum >> 16);
    sum as u16
}

/// Incrementally update a 16-bit one's-complement checksum (RFC 1624
/// equation 3). Inputs and outputs are on-the-wire / host-neutral
/// 16-bit fields; callers that sourced bytes from a packet typically
/// pass raw packet bytes directly.
#[inline(always)]
fn csum16_update(old_csum: u16, old_field: u16, new_field: u16) -> u16 {
    // ~HC + ~m + m', as u32 to preserve carries through the folds.
    let sum: u32 = u32::from(!old_csum) + u32::from(!old_field) + u32::from(new_field);
    !fold32(sum)
}

/// Incremental checksum for a single 32-bit field change. Applied twice
/// under the hood.
#[inline(always)]
fn csum16_update_u32(old_csum: u16, old_field: u32, new_field: u32) -> u16 {
    let c1 = csum16_update(old_csum, (old_field >> 16) as u16, (new_field >> 16) as u16);
    csum16_update(c1, old_field as u16, new_field as u16)
}

/// Update for a 128-bit IPv6 address change — eight u16 lanes.
#[inline(always)]
fn csum16_update_v6(old_csum: u16, old_addr: &[u8; 16], new_addr: &[u8; 16]) -> u16 {
    let mut c = old_csum;
    let mut i = 0;
    while i < 16 {
        let o = (u16::from(old_addr[i]) << 8) | u16::from(old_addr[i + 1]);
        let n = (u16::from(new_addr[i]) << 8) | u16::from(new_addr[i + 1]);
        c = csum16_update(c, o, n);
        i += 2;
    }
    c
}

// ---------------------------------------------------------------------------
// Entry point.
// ---------------------------------------------------------------------------

#[xdp]
pub fn lb_xdp(ctx: XdpContext) -> u32 {
    match try_lb_xdp(&ctx) {
        Ok(action) => action,
        Err(()) => {
            incr_stat(STAT_PARSE_FAIL);
            xdp_action::XDP_PASS
        }
    }
}

fn try_lb_xdp(ctx: &XdpContext) -> Result<u32, ()> {
    // --- Ethernet + optional single VLAN tag -----------------------------
    let eth = unsafe { ptr_at::<EthHdr>(ctx, 0).ok_or(())? };
    // SAFETY: eth validated; packed field read.
    let eth_type = u16::from_be(unsafe {
        core::ptr::read_unaligned(core::ptr::addr_of!((*eth).ether_type))
    });

    let (l3_offset, ether_type) = if eth_type == ETH_P_8021Q {
        incr_stat(STAT_VLAN);
        let vlan = unsafe { ptr_at::<VlanHdr>(ctx, ETH_HDR_LEN).ok_or(())? };
        // SAFETY: packed field read.
        let inner_type = u16::from_be(unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!((*vlan).ether_type))
        });
        (ETH_HDR_LEN + VLAN_HDR_LEN, inner_type)
    } else {
        (ETH_HDR_LEN, eth_type)
    };

    match ether_type {
        ETH_P_IP => handle_ipv4(ctx, l3_offset),
        ETH_P_IPV6 => handle_ipv6(ctx, l3_offset),
        _ => {
            incr_stat(STAT_PASS);
            Ok(xdp_action::XDP_PASS)
        }
    }
}

// ---------------------------------------------------------------------------
// IPv4 path.
// ---------------------------------------------------------------------------

fn handle_ipv4(ctx: &XdpContext, l3_offset: usize) -> Result<u32, ()> {
    let ip = unsafe { ptr_at::<Ipv4Hdr>(ctx, l3_offset).ok_or(())? };
    // SAFETY: packed field reads.
    let version_ihl = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*ip).version_ihl)) };
    let ihl_words = (version_ihl & 0x0F) as usize;
    if ihl_words < 5 {
        return Err(());
    }
    let ip_hdr_len = ihl_words * 4;
    let protocol = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*ip).protocol)) };
    let src_addr = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*ip).src)) };
    let dst_addr = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*ip).dst)) };

    // --- ACL via LPM trie ------------------------------------------------
    // Key data is u32 in network byte order; prefix_len is CIDR bits.
    // A /32 lookup returns the entry that matches the most specific
    // deny prefix (if any).
    let lpm_key = LpmKey::<u32>::new(32, src_addr);
    if ACL_DENY_TRIE.get(&lpm_key).is_some() {
        incr_stat(STAT_DROP);
        return Ok(xdp_action::XDP_DROP);
    }

    let l4_offset = l3_offset + ip_hdr_len;
    let (src_port, dst_port) = match protocol {
        IPPROTO_TCP => {
            let tcp = unsafe { ptr_at::<TcpHdr>(ctx, l4_offset).ok_or(())? };
            // SAFETY: packed field reads.
            let sp = u16::from_be(unsafe {
                core::ptr::read_unaligned(core::ptr::addr_of!((*tcp).src_port))
            });
            let dp = u16::from_be(unsafe {
                core::ptr::read_unaligned(core::ptr::addr_of!((*tcp).dst_port))
            });
            (sp, dp)
        }
        IPPROTO_UDP => {
            let udp = unsafe { ptr_at::<UdpHdr>(ctx, l4_offset).ok_or(())? };
            // SAFETY: packed field reads.
            let sp = u16::from_be(unsafe {
                core::ptr::read_unaligned(core::ptr::addr_of!((*udp).src_port))
            });
            let dp = u16::from_be(unsafe {
                core::ptr::read_unaligned(core::ptr::addr_of!((*udp).dst_port))
            });
            (sp, dp)
        }
        _ => {
            incr_stat(STAT_PASS);
            return Ok(xdp_action::XDP_PASS);
        }
    };

    if unsafe { L7_PORTS.get(&dst_port) }.is_some() {
        incr_stat(STAT_L7);
        return Ok(xdp_action::XDP_PASS);
    }

    let key = FlowKey {
        src_addr,
        dst_addr,
        src_port: src_port.to_be(),
        dst_port: dst_port.to_be(),
        protocol,
        _pad: [0; 3],
    };
    // SAFETY: CONNTRACK.get reads atomically; pointer is valid for the
    // duration of this probe. Copy the BackendEntry into a local to end
    // the borrow before we start mutating the packet.
    let entry: BackendEntry = match unsafe { CONNTRACK.get(&key) } {
        Some(v) => *v,
        None => {
            incr_stat(STAT_PASS);
            return Ok(xdp_action::XDP_PASS);
        }
    };
    incr_stat(STAT_CT_HIT_V4);

    // --- Rewrite: MAC, dst IP, dst port, L3 + L4 checksums ---------------
    rewrite_v4(ctx, l3_offset, ip_hdr_len, protocol, dst_addr, &entry)?;
    incr_stat(STAT_TX_V4);
    Ok(xdp_action::XDP_TX)
}

#[inline(always)]
fn rewrite_v4(
    ctx: &XdpContext,
    l3_offset: usize,
    ip_hdr_len: usize,
    protocol: u8,
    old_dst_ip: u32,
    entry: &BackendEntry,
) -> Result<(), ()> {
    // MAC rewrite.
    let eth_m = unsafe { ptr_at_mut::<EthHdr>(ctx, 0).ok_or(())? };
    // SAFETY: eth_m validated.
    unsafe {
        core::ptr::write_unaligned(core::ptr::addr_of_mut!((*eth_m).dst), entry.backend_mac);
        core::ptr::write_unaligned(core::ptr::addr_of_mut!((*eth_m).src), entry.src_mac);
    }

    // IPv4 dst + L3 checksum.
    let ip_m = unsafe { ptr_at_mut::<Ipv4Hdr>(ctx, l3_offset).ok_or(())? };
    // SAFETY: packed field reads.
    let old_check = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*ip_m).check)) };
    let new_check = csum16_update_u32(
        u16::from_be(old_check),
        u32::from_be(old_dst_ip),
        u32::from_be(entry.backend_ip),
    )
    .to_be();
    // SAFETY: packed field writes on validated pointer.
    unsafe {
        core::ptr::write_unaligned(core::ptr::addr_of_mut!((*ip_m).dst), entry.backend_ip);
        core::ptr::write_unaligned(core::ptr::addr_of_mut!((*ip_m).check), new_check);
    }

    // L4 dst port + L4 checksum (covers pseudo-header that includes
    // dst IP, so dst-IP change also participates).
    let l4_offset = l3_offset + ip_hdr_len;
    match protocol {
        IPPROTO_TCP => {
            let tcp_m = unsafe { ptr_at_mut::<TcpHdrRW>(ctx, l4_offset).ok_or(())? };
            // SAFETY: packed field reads/writes on validated pointer.
            unsafe {
                let old_dst_port =
                    core::ptr::read_unaligned(core::ptr::addr_of!((*tcp_m).dst_port));
                let old_check =
                    core::ptr::read_unaligned(core::ptr::addr_of!((*tcp_m).check));
                let mut c = u16::from_be(old_check);
                // Pseudo-header dst IP change.
                c = csum16_update_u32(c, u32::from_be(old_dst_ip), u32::from_be(entry.backend_ip));
                // Dst port change.
                c = csum16_update(c, u16::from_be(old_dst_port), entry.backend_port.swap_bytes());
                core::ptr::write_unaligned(
                    core::ptr::addr_of_mut!((*tcp_m).dst_port),
                    entry.backend_port,
                );
                core::ptr::write_unaligned(
                    core::ptr::addr_of_mut!((*tcp_m).check),
                    c.to_be(),
                );
            }
        }
        IPPROTO_UDP => {
            let udp_m = unsafe { ptr_at_mut::<UdpHdr>(ctx, l4_offset).ok_or(())? };
            // SAFETY: packed field reads/writes on validated pointer.
            unsafe {
                let old_dst_port =
                    core::ptr::read_unaligned(core::ptr::addr_of!((*udp_m).dst_port));
                let old_check =
                    core::ptr::read_unaligned(core::ptr::addr_of!((*udp_m).check));
                // UDP checksum == 0 means "not computed" — leave as-is.
                if old_check != 0 {
                    let mut c = u16::from_be(old_check);
                    c = csum16_update_u32(
                        c,
                        u32::from_be(old_dst_ip),
                        u32::from_be(entry.backend_ip),
                    );
                    c = csum16_update(
                        c,
                        u16::from_be(old_dst_port),
                        entry.backend_port.swap_bytes(),
                    );
                    core::ptr::write_unaligned(
                        core::ptr::addr_of_mut!((*udp_m).check),
                        c.to_be(),
                    );
                }
                core::ptr::write_unaligned(
                    core::ptr::addr_of_mut!((*udp_m).dst_port),
                    entry.backend_port,
                );
            }
        }
        _ => return Err(()),
    }
    Ok(())
}

/// Extended TCP header struct for the rewrite path: we also need the
/// checksum at offset 16. Pillar 4b-2 does not touch URG/ACK or any
/// other fields.
#[repr(C, packed(2))]
struct TcpHdrRW {
    src_port: u16,
    dst_port: u16,
    _seq: u32,
    _ack: u32,
    _offset_flags: u16,
    _window: u16,
    check: u16,
    _urg_ptr: u16,
}

// ---------------------------------------------------------------------------
// IPv6 path.
// ---------------------------------------------------------------------------

fn handle_ipv6(ctx: &XdpContext, l3_offset: usize) -> Result<u32, ()> {
    let ip = unsafe { ptr_at::<Ipv6Hdr>(ctx, l3_offset).ok_or(())? };
    // SAFETY: packed field reads.
    let mut next_hdr =
        unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*ip).next_header)) };
    let src_addr = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*ip).src)) };
    let dst_addr = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*ip).dst)) };

    let mut off = l3_offset + IPV6_HDR_LEN;

    // Skip at most 2 extension headers (Hop-by-Hop, Routing). Verifier
    // will not accept an unbounded loop; a fixed small count is fine.
    let mut extensions_consumed: u32 = 0;
    while extensions_consumed < 2
        && (next_hdr == IPPROTO_HOPOPTS || next_hdr == IPPROTO_ROUTING)
    {
        let ext = unsafe { ptr_at::<Ipv6ExtHdr>(ctx, off).ok_or(())? };
        // SAFETY: packed field reads.
        let (nh, len) = unsafe {
            (
                core::ptr::read_unaligned(core::ptr::addr_of!((*ext).next_header)),
                core::ptr::read_unaligned(core::ptr::addr_of!((*ext).hdr_ext_len)),
            )
        };
        // Total ext-header length = (hdr_ext_len + 1) * 8.
        off += (usize::from(len) + 1) * 8;
        next_hdr = nh;
        extensions_consumed += 1;
    }
    if next_hdr == IPPROTO_HOPOPTS || next_hdr == IPPROTO_ROUTING {
        // More extension headers than we handle → pass to kernel.
        incr_stat(STAT_V6_EXT_UNSUPPORTED);
        return Ok(xdp_action::XDP_PASS);
    }

    let (src_port, dst_port) = match next_hdr {
        IPPROTO_TCP => {
            let tcp = unsafe { ptr_at::<TcpHdr>(ctx, off).ok_or(())? };
            // SAFETY: packed field reads.
            let sp = u16::from_be(unsafe {
                core::ptr::read_unaligned(core::ptr::addr_of!((*tcp).src_port))
            });
            let dp = u16::from_be(unsafe {
                core::ptr::read_unaligned(core::ptr::addr_of!((*tcp).dst_port))
            });
            (sp, dp)
        }
        IPPROTO_UDP => {
            let udp = unsafe { ptr_at::<UdpHdr>(ctx, off).ok_or(())? };
            // SAFETY: packed field reads.
            let sp = u16::from_be(unsafe {
                core::ptr::read_unaligned(core::ptr::addr_of!((*udp).src_port))
            });
            let dp = u16::from_be(unsafe {
                core::ptr::read_unaligned(core::ptr::addr_of!((*udp).dst_port))
            });
            (sp, dp)
        }
        _ => {
            incr_stat(STAT_PASS);
            return Ok(xdp_action::XDP_PASS);
        }
    };

    if unsafe { L7_PORTS.get(&dst_port) }.is_some() {
        incr_stat(STAT_L7);
        return Ok(xdp_action::XDP_PASS);
    }

    let key = FlowKeyV6 {
        src_addr,
        dst_addr,
        src_port: src_port.to_be(),
        dst_port: dst_port.to_be(),
        protocol: next_hdr,
        _pad: [0; 3],
    };
    let entry: BackendEntryV6 = match unsafe { CONNTRACK_V6.get(&key) } {
        Some(v) => *v,
        None => {
            incr_stat(STAT_PASS);
            return Ok(xdp_action::XDP_PASS);
        }
    };
    incr_stat(STAT_CT_HIT_V6);

    rewrite_v6(ctx, l3_offset, off, next_hdr, &dst_addr, &entry)?;
    incr_stat(STAT_TX_V6);
    Ok(xdp_action::XDP_TX)
}

#[inline(always)]
fn rewrite_v6(
    ctx: &XdpContext,
    l3_offset: usize,
    l4_offset: usize,
    protocol: u8,
    old_dst_ip: &[u8; 16],
    entry: &BackendEntryV6,
) -> Result<(), ()> {
    // MAC rewrite.
    let eth_m = unsafe { ptr_at_mut::<EthHdr>(ctx, 0).ok_or(())? };
    // SAFETY: packed writes on validated pointer.
    unsafe {
        core::ptr::write_unaligned(core::ptr::addr_of_mut!((*eth_m).dst), entry.backend_mac);
        core::ptr::write_unaligned(core::ptr::addr_of_mut!((*eth_m).src), entry.src_mac);
    }

    // IPv6 dst (no L3 checksum in IPv6).
    let ip_m = unsafe { ptr_at_mut::<Ipv6Hdr>(ctx, l3_offset).ok_or(())? };
    // SAFETY: packed write.
    unsafe {
        core::ptr::write_unaligned(core::ptr::addr_of_mut!((*ip_m).dst), entry.backend_ip);
    }

    // L4 checksum update for both (a) 128-bit IPv6 dst in the
    // pseudo-header and (b) the 16-bit dst port.
    match protocol {
        IPPROTO_TCP => {
            let tcp_m = unsafe { ptr_at_mut::<TcpHdrRW>(ctx, l4_offset).ok_or(())? };
            // SAFETY: packed reads/writes on validated pointer.
            unsafe {
                let old_dst_port =
                    core::ptr::read_unaligned(core::ptr::addr_of!((*tcp_m).dst_port));
                let old_check =
                    core::ptr::read_unaligned(core::ptr::addr_of!((*tcp_m).check));
                let mut c = u16::from_be(old_check);
                c = csum16_update_v6(c, old_dst_ip, &entry.backend_ip);
                c = csum16_update(c, u16::from_be(old_dst_port), entry.backend_port.swap_bytes());
                core::ptr::write_unaligned(
                    core::ptr::addr_of_mut!((*tcp_m).dst_port),
                    entry.backend_port,
                );
                core::ptr::write_unaligned(
                    core::ptr::addr_of_mut!((*tcp_m).check),
                    c.to_be(),
                );
            }
        }
        IPPROTO_UDP => {
            let udp_m = unsafe { ptr_at_mut::<UdpHdr>(ctx, l4_offset).ok_or(())? };
            // SAFETY: packed reads/writes on validated pointer.
            unsafe {
                let old_dst_port =
                    core::ptr::read_unaligned(core::ptr::addr_of!((*udp_m).dst_port));
                let old_check =
                    core::ptr::read_unaligned(core::ptr::addr_of!((*udp_m).check));
                // IPv6 requires a non-zero UDP checksum; we only rewrite if
                // one was already computed.
                if old_check != 0 {
                    let mut c = u16::from_be(old_check);
                    c = csum16_update_v6(c, old_dst_ip, &entry.backend_ip);
                    c = csum16_update(
                        c,
                        u16::from_be(old_dst_port),
                        entry.backend_port.swap_bytes(),
                    );
                    core::ptr::write_unaligned(
                        core::ptr::addr_of_mut!((*udp_m).check),
                        c.to_be(),
                    );
                }
                core::ptr::write_unaligned(
                    core::ptr::addr_of_mut!((*udp_m).dst_port),
                    entry.backend_port,
                );
            }
        }
        _ => return Err(()),
    }
    Ok(())
}

// Required by the BPF linker: panic handler for no_std + panic=abort.
#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    loop {}
}
