//! L4 XDP data plane for TCP/UDP load balancing.
//!
//! IPv4: parse eth (optionally stripping ONE 802.1Q tag — QinQ is deferred), ACL_DENY_TRIE on src IP, ports, CONNTRACK; on a hit rewrite MAC + dst IP + dst port with RFC 1624
//! incremental checksum updates and `XDP_TX`, on a miss `XDP_PASS` so userspace picks a backend. IPv6 mirrors it (up to two extension headers; no L3 checksum, but the L4 checksum
//! covers the pseudo-header so RFC 1624 still applies).
//!
//! On ANY bounds-check failure the program returns `XDP_PASS` — never `XDP_DROP` on a parse failure.

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

// EBPF-2-01/02: the kernel `BPF_PROG_LOAD` syscall reads `bpf_attr.license` from this ELF section; declaring it explicitly drops the dependency on aya-obj's "GPL" default, and `no_mangle` keeps bpf-linker's DCE from stripping the symbol.
#[unsafe(link_section = "license")]
#[unsafe(no_mangle)]
pub static LICENSE: [u8; 4] = *b"GPL\0";

use core::mem;

use aya_ebpf::{
    bindings::xdp_action,
    macros::{map, xdp},
    // EBPF-2-03: CONNTRACK/CONNTRACK_V6 are LruHashMap so the kernel evicts the oldest entry under flood instead of returning ENOMEM. L7_PORTS is config-managed, never flood-pressured, so it stays a plain HashMap.
    maps::{HashMap, LpmTrie, LruHashMap, PerCpuArray, lpm_trie::Key as LpmKey},
    programs::XdpContext,
};

// Wire constants and header shapes. Repr(C, packed(2)) pins kernel layout.

const ETH_P_IP: u16 = 0x0800;
const ETH_P_IPV6: u16 = 0x86DD;
const ETH_P_8021Q: u16 = 0x8100;
const IPPROTO_TCP: u8 = 6;
const IPPROTO_UDP: u8 = 17;
const IPPROTO_HOPOPTS: u8 = 0;
const IPPROTO_ROUTING: u8 = 43;
/// ROUND8-L4-08: IPv6 Fragment Extension Header (RFC 2460 §4.5).
const IPPROTO_FRAGMENT: u8 = 44;

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
    _ver_tc_fl: u32,
    payload_len: u16,
    next_header: u8,
    hop_limit: u8,
    src: [u8; 16],
    dst: [u8; 16],
}

/// IPv6 extension-header shape; only `next_header` + `hdr_ext_len` are consulted. `hdr_ext_len` is in 8-byte units NOT counting the first 8 bytes — the kernel convention.
#[repr(C, packed(2))]
struct Ipv6ExtHdr {
    next_header: u8,
    hdr_ext_len: u8,
}

#[repr(C, packed(2))]
struct TcpHdr {
    src_port: u16,
    dst_port: u16,
    _seq: u32,
    _ack: u32,
    _data_offset_ns: u8,
    /// CWR | ECE | URG | ACK | PSH | RST | SYN | FIN — read by ROUND8-L4-02 for state-aware conntrack pruning.
    flags: u8,
    _window: u16,
}

/// ROUND8-L4-02: TCP control bits for the state-aware conntrack prune (Cilium `bpf/lib/conntrack.h`). Pure LRU is vulnerable to a sliding-RST replay: an adversary spraying RST/FIN
/// across evicted flows fills the LRU's young end and pushes live flows out. Pruning on RST and on FIN-ACK tracks TCP-FSM reality without a verifier-costly full FSM.
const TCP_FLAG_FIN: u8 = 0x01;
const TCP_FLAG_RST: u8 = 0x04;
const TCP_FLAG_ACK: u8 = 0x10;

#[repr(C, packed(2))]
struct UdpHdr {
    src_port: u16,
    dst_port: u16,
    len: u16,
    check: u16,
}


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

/// IPv6 flow 5-tuple. Padded to a size the verifier likes (16+16+2+2+1+3 = 40 bytes).
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

/// Conntrack value for IPv4 flows. Carries the full rewrite state so the BPF program needs no secondary lookup to run an `XDP_TX`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BackendEntry {
    pub backend_idx: u32,
    pub backend_ip: u32,
    pub backend_port: u16,
    pub _pad: u16,
    pub backend_mac: [u8; 6],
    pub src_mac: [u8; 6],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct BackendEntryV6 {
    pub backend_idx: u32,
    pub backend_ip: [u8; 16],
    pub backend_port: u16,
    pub _pad: u16,
    pub backend_mac: [u8; 6],
    pub src_mac: [u8; 6],
}

// ROUND8-L4-04: atomic per-VIP backend-table publication (Unimog / l4drop D1). `BackendTable` is ONE map value, so a single `bpf_map_update_elem` of the whole struct is atomic w.r.t.
// concurrent lookups — a reader sees the entire old or entire new table, never a torn merge. That single-syscall publication IS the fix for the previous N-syscall window.
//
// SCOPE: this freezes the layout + the userspace publish/daisy-chain contract only. The verifier-heavy hot-path read (per-packet `BACKENDS_V4[vip]` + bounded `entries[hash % count]`
// + generation compare) lands with consistent-hash selection; wiring it now would force a verifier-log re-capture for a path no production flow exercises yet.

/// ROUND8-L4-04: verifier-tractable ceiling on backends per VIP. More needs partitioning or Maglev consistent hashing (`audit/deferred.md`).
pub const MAX_BACKENDS_PER_VIP: usize = 64;

/// ROUND8-L4-04: per-VIP backend table, published atomically as a single map value (Unimog D1). `generation` increments on every publish; a CT entry whose remembered generation differs is
/// in the transitional window and consults `previous_entries` (Unimog lesson 3), so in-flight flows reach the previous backend instead of being stranded.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BackendTable {
    /// Monotonic publication counter (wraps at u32::MAX — only equality matters, not ordering).
    pub generation: u32,
    pub count: u32,
    pub entries: [BackendEntry; MAX_BACKENDS_PER_VIP],
    /// Daisy-chain: the previous generation's live count. Zero outside the transitional window.
    pub previous_count: u32,
    pub _pad: u32,
    /// Daisy-chain: the previous generation's backends, so in-flight flows keep their backend.
    pub previous_entries: [BackendEntry; MAX_BACKENDS_PER_VIP],
}

#[map(name = "backends_v4")]
static BACKENDS_V4: HashMap<u32, BackendTable> =
    HashMap::<u32, BackendTable>::with_max_entries(1024, 0);

/// ROUND8-L4-04: verifier-safe, behaviorally-inert reference to `BACKENDS_V4`, called once on the IPv4 CT-miss path so (1) bpf-linker DCE keeps the map + its BTF alive and (2) a lookup
/// that observes a published table proves the single-syscall swap is visible to the data plane. It deliberately does NOT select a backend or mutate CONNTRACK — that is the deferred
/// verifier-heavy piece. Without this the map reads as dead code.
#[inline(always)]
fn backend_table_published(vip: u32) -> bool {
    match unsafe { BACKENDS_V4.get(&vip) } {
        Some(t) => t.count > 0,
        None => false,
    }
}

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
/// ROUND8-L4-01: a conntrack hit whose backend_ip/backend_port is zero is an unpopulated controller entry. PASS (not drop) so the kernel still routes it; the counter is the signal.
const STAT_BACKEND_UNPOPULATED: u32 = 10;
/// ROUND8-L4-08: IPv4 fragment seen — passed to the kernel for reassembly (Katran/Cilium design: no in-XDP reassembly).
const STAT_V4_FRAGMENT: u32 = 11;
const STAT_V6_FRAGMENT: u32 = 12;
/// ROUND8-L4-02: a TCP RST evicted its conntrack entry. The packet still goes `XDP_PASS` so the RST reaches the peer end-to-end; only flow *tracking* stops.
const STAT_CT_RST_PRUNE: u32 = 13;
/// ROUND8-L4-02: a TCP FIN-ACK evicted its conntrack entry. The packet is still forwarded (`XDP_TX`), but the slot is freed so a replay cannot pin LRU capacity.
const STAT_CT_FIN_PRUNE: u32 = 14;
/// ROUND8-L4-03: a new flow (conntrack miss) was rate-capped under a SYN flood. Per Katran `is_under_flood()`, above the per-CPU cap the miss path short-circuits to `XDP_PASS` WITHOUT
/// signalling userspace to populate CONNTRACK, so the LRU stays stable for established (CT-hit) flows instead of being thrashed by the attacker's unique 5-tuples.
const STAT_NEW_FLOW_RATE_CAP: u32 = 15;

// EBPF-2-03: LRU_HASH evicts the oldest entry under flood instead of returning ENOMEM at insert, closing the flow-spray DoS that starved legitimate new connections.
//
// EBPF-2-05: the explicit lowercase `name = …` decouples the on-disk pin filename from Rust identifier churn — aya defaults it to the uppercased identifier, so a rename of the Rust static would force a pin rename + state loss.
#[map(name = "conntrack")]
static CONNTRACK: LruHashMap<FlowKey, BackendEntry> =
    LruHashMap::<FlowKey, BackendEntry>::with_max_entries(1_000_000, 0);

#[map(name = "conntrack_v6")]
static CONNTRACK_V6: LruHashMap<FlowKeyV6, BackendEntryV6> =
    LruHashMap::<FlowKeyV6, BackendEntryV6>::with_max_entries(512_000, 0);

#[map(name = "l7_ports")]
static L7_PORTS: HashMap<u16, u8> = HashMap::<u16, u8>::with_max_entries(256, 0);

/// IPv4 deny ACL as a longest-prefix-match trie. Key data is the address in network byte order; `prefix_len` is the CIDR mask length.
#[map(name = "acl_deny_trie")]
static ACL_DENY_TRIE: LpmTrie<u32, u32> = LpmTrie::<u32, u32>::with_max_entries(100_000, 0);

#[map(name = "stats")]
static STATS: PerCpuArray<u64> = PerCpuArray::<u64>::with_max_entries(32, 0);

// ROUND8-L4-03: per-CPU new-flow-rate tracker (Katran `is_under_flood()` lesson 4). Under a SYN flood an attacker sprays millions of unique 5-tuples/sec; each is a CT miss the control
// plane would answer with a `bpf_map_update_elem`, making every established flow an LRU eviction loser. The cap is per-CPU (XDP runs one instance per RX queue, so no cross-CPU
// coherence cost) over a 1 s window keyed off `bpf_ktime_get_ns()`.
//
// IMPORTANT: this changes the BPF source, so the verifier-log baselines under `audit/ebpf/verifier-logs/*.committed` must be refreshed by the next CI matrix run.

/// Per-CPU sliding-window counter for the new-flow-rate cap.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RateWindow {
    pub window_start_ns: u64,
    pub flows_this_window: u32,
    pub _pad: u32,
}

#[map(name = "new_flow_rate")]
static NEW_FLOW_RATE: PerCpuArray<RateWindow> = PerCpuArray::<RateWindow>::with_max_entries(1, 0);

/// Runtime-tunable per-CPU new-flow cap — a one-entry per-CPU array the hot path reads once per CT-miss (verifier-cheap). A zero value means the operator DISABLED the cap.
#[map(name = "new_flow_cap_cfg")]
static NEW_FLOW_CAP_CFG: PerCpuArray<u32> = PerCpuArray::<u32>::with_max_entries(1, 0);

/// ROUND8-L4-03: Katran `MAX_CONN_RATE` parity — the compile-time fallback for the window before userspace first writes `NEW_FLOW_CAP_CFG`. Since 0 in the cfg map means "operator
/// disabled", not-yet-written is distinguished from disabled by consulting this fallback ONLY when the slot is unreadable.
const DEFAULT_NEW_FLOW_CAP_PER_CPU: u32 = 125_000;

const RATE_WINDOW_NS: u64 = 1_000_000_000;

/// ROUND8-L4-03: true when this CPU has admitted more than the configured cap of new flows in the current 1 s window (Katran `is_under_flood()`: per-CPU window, reset on rollover,
/// increment-then-compare). Called only on the CT-MISS path — an established flow is never rate-capped, which is the whole point.
#[inline(always)]
fn is_under_flood() -> bool {
    let cap = match NEW_FLOW_CAP_CFG.get_ptr(0) {
        // SAFETY: aya returned a non-null pointer for this CPU's slot.
        Some(p) => unsafe { *p },
        None => DEFAULT_NEW_FLOW_CAP_PER_CPU,
    };
    // Cap of 0 = operator disabled the rate limiter entirely.
    if cap == 0 {
        return false;
    }
    let Some(slot) = NEW_FLOW_RATE.get_ptr_mut(0) else {
        // Map unreadable — fail OPEN (do not drop legitimate traffic because telemetry state is unavailable).
        return false;
    };
    // SAFETY: aya returned a non-null pointer for this CPU's slot;
    // per-CPU array element is exclusively owned by this CPU.
    let w = unsafe { &mut *slot };
    let now = unsafe { aya_ebpf::helpers::bpf_ktime_get_ns() };
    if now.wrapping_sub(w.window_start_ns) > RATE_WINDOW_NS {
        w.window_start_ns = now;
        w.flows_this_window = 0;
    }
    w.flows_this_window = w.flows_this_window.saturating_add(1);
    w.flows_this_window > cap
}


// ROUND8-L4-09: every addition in `ptr_at`/`ptr_at_mut` uses `checked_add`. Callers pass compile-time-known offsets today, but the verifier evolves between LTS kernels and aya #1562
// documented scalar/pointer re-ordering on recent rustc/LLVM. This guards the CVE-2022-23222-class bounds-check elision for any future runtime-controlled offset; `checked_add` lowers to
// `llvm.uadd.with.overflow.i64`, which the verifier handles on 5.15+.
//
// IMPORTANT: changing this file obliges a refresh of the verifier-log baselines under `audit/ebpf/verifier-logs/*.log.committed`.

#[inline(always)]
unsafe fn ptr_at<T>(ctx: &XdpContext, offset: usize) -> Option<*const T> {
    let start = ctx.data();
    let end = ctx.data_end();
    let len = mem::size_of::<T>();
    // Checked arithmetic so the bounds-check cannot be elided via wrap-around (aya #1562 / CVE-2022-23222 class).
    let needed = start.checked_add(offset)?.checked_add(len)?;
    if needed > end {
        return None;
    }
    let addr = start.checked_add(offset)?;
    // SAFETY: bounds validated; pointer is within [start, end).
    Some(addr as *const T)
}

#[inline(always)]
unsafe fn ptr_at_mut<T>(ctx: &XdpContext, offset: usize) -> Option<*mut T> {
    let start = ctx.data();
    let end = ctx.data_end();
    let len = mem::size_of::<T>();
    let needed = start.checked_add(offset)?.checked_add(len)?;
    if needed > end {
        return None;
    }
    let addr = start.checked_add(offset)?;
    // SAFETY: bounds validated.
    Some(addr as *mut T)
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

// RFC 1624 incremental checksum helpers. §3: HC' = ~(~HC + ~m + m'), where HC is the old checksum, m the old 16-bit field and m' the new one. Operates on already-folded
// ones-complement sums; carries are folded at the end.

#[inline(always)]
fn fold32(mut sum: u32) -> u16 {
    // Two folds suffice for any u32 input.
    sum = (sum & 0xFFFF) + (sum >> 16);
    sum = (sum & 0xFFFF) + (sum >> 16);
    sum as u16
}

/// Incrementally update a 16-bit one's-complement checksum (RFC 1624 eq. 3). Fields are host-neutral, so callers may pass raw packet bytes directly.
#[inline(always)]
fn csum16_update(old_csum: u16, old_field: u16, new_field: u16) -> u16 {
    // ~HC + ~m + m', as u32 to preserve carries through the folds.
    let sum: u32 = u32::from(!old_csum) + u32::from(!old_field) + u32::from(new_field);
    !fold32(sum)
}

#[inline(always)]
fn csum16_update_u32(old_csum: u16, old_field: u32, new_field: u32) -> u16 {
    let c1 = csum16_update(old_csum, (old_field >> 16) as u16, (new_field >> 16) as u16);
    csum16_update(c1, old_field as u16, new_field as u16)
}

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

    // ROUND8-L4-08: RFC 791 §3.1 — `frag_off` bit 14 is MF and bits 0..12 are the offset in 8-byte units. MF==1 or offset>0 means this is not a complete datagram: pass to the kernel
    // for reassembly (Katran/Cilium design).
    let frag_off_be =
        unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*ip).frag_off)) };
    let frag_off = u16::from_be(frag_off_be);
    if (frag_off & 0x3FFF) != 0 {
        incr_stat(STAT_V4_FRAGMENT);
        return Ok(xdp_action::XDP_PASS);
    }

    // LPM key data is u32 in network byte order; a /32 lookup returns the most specific matching deny prefix.
    let lpm_key = LpmKey::<u32>::new(32, src_addr);
    if ACL_DENY_TRIE.get(&lpm_key).is_some() {
        incr_stat(STAT_DROP);
        return Ok(xdp_action::XDP_DROP);
    }

    let l4_offset = l3_offset + ip_hdr_len;
    // ROUND8-L4-02: parse TCP flags alongside the ports so the prune branch fires BEFORE the rewrite path.
    let (src_port, dst_port, tcp_flags) = match protocol {
        IPPROTO_TCP => {
            let tcp = unsafe { ptr_at::<TcpHdr>(ctx, l4_offset).ok_or(())? };
            // SAFETY: packed field reads.
            let sp = u16::from_be(unsafe {
                core::ptr::read_unaligned(core::ptr::addr_of!((*tcp).src_port))
            });
            let dp = u16::from_be(unsafe {
                core::ptr::read_unaligned(core::ptr::addr_of!((*tcp).dst_port))
            });
            let flags =
                unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*tcp).flags)) };
            (sp, dp, flags)
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
            (sp, dp, 0u8)
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

    // ROUND8-L4-02: prune BEFORE the lookup-and-rewrite hot path. The LRU returns the entry even on RST/FIN-ACK and we always want the slot freed, so a sliding-RST replay cannot pin LRU
    // capacity. The packet goes XDP_PASS on RST and XDP_TX on FIN-ACK (the last FIN-ACK still needs rewriting and forwarding).
    if protocol == IPPROTO_TCP && (tcp_flags & TCP_FLAG_RST) != 0 {
        // The Result is discarded: "no such key" is the steady state for unrelated RST sprays.
        let _ = CONNTRACK.remove(&key);
        incr_stat(STAT_CT_RST_PRUNE);
        incr_stat(STAT_PASS);
        return Ok(xdp_action::XDP_PASS);
    }

    // SAFETY: CONNTRACK.get reads atomically; pointer is valid for the
    // duration of this probe. Copy the BackendEntry into a local to end
    // the borrow before we start mutating the packet.
    let entry: BackendEntry = match unsafe { CONNTRACK.get(&key) } {
        Some(v) => *v,
        None => {
            // ROUND8-L4-04: behaviorally-inert touch that keeps the map + BTF alive for userspace `publish_backends_v4` and proves the publication is visible here.
            let _table_ready = backend_table_published(dst_addr);
            // ROUND8-L4-03: a CT miss is a NEW flow — the attacker's lever under a SYN flood. Above the per-CPU cap, short-circuit to XDP_PASS WITHOUT the STAT_PASS signal the control loop reads as "populate CT". CT-hit flows above are unaffected.
            if is_under_flood() {
                incr_stat(STAT_NEW_FLOW_RATE_CAP);
                return Ok(xdp_action::XDP_PASS);
            }
            incr_stat(STAT_PASS);
            return Ok(xdp_action::XDP_PASS);
        }
    };
    incr_stat(STAT_CT_HIT_V4);

    // ROUND8-L4-01: sentinel guard — a zero backend_ip/backend_port is a not-yet-populated controller entry, so XDP_PASS keeps the kernel stack as the fallback.
    if entry.backend_ip == 0 || entry.backend_port == 0 {
        incr_stat(STAT_BACKEND_UNPOPULATED);
        return Ok(xdp_action::XDP_PASS);
    }

    rewrite_v4(ctx, l3_offset, ip_hdr_len, protocol, dst_addr, &entry)?;
    incr_stat(STAT_TX_V4);

    // ROUND8-L4-02: FIN-ACK prune AFTER the rewrite — the last FIN-ACK must still reach the backend, but the slot is freed so a replay cannot revive a closed flow.
    if protocol == IPPROTO_TCP
        && (tcp_flags & TCP_FLAG_FIN) != 0
        && (tcp_flags & TCP_FLAG_ACK) != 0
    {
        let _ = CONNTRACK.remove(&key);
        incr_stat(STAT_CT_FIN_PRUNE);
    }

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
    let eth_m = unsafe { ptr_at_mut::<EthHdr>(ctx, 0).ok_or(())? };
    // SAFETY: eth_m validated.
    unsafe {
        core::ptr::write_unaligned(core::ptr::addr_of_mut!((*eth_m).dst), entry.backend_mac);
        core::ptr::write_unaligned(core::ptr::addr_of_mut!((*eth_m).src), entry.src_mac);
    }

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

    // L4 dst port + checksum (the pseudo-header covers dst IP, so the IP change participates).
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
                c = csum16_update_u32(c, u32::from_be(old_dst_ip), u32::from_be(entry.backend_ip));
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

/// Extended TCP header for the rewrite path — we also need the checksum at offset 16.
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


fn handle_ipv6(ctx: &XdpContext, l3_offset: usize) -> Result<u32, ()> {
    let ip = unsafe { ptr_at::<Ipv6Hdr>(ctx, l3_offset).ok_or(())? };
    // SAFETY: packed field reads.
    let mut next_hdr =
        unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*ip).next_header)) };
    let src_addr = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*ip).src)) };
    let dst_addr = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*ip).dst)) };

    let mut off = l3_offset + IPV6_HDR_LEN;

    // Skip at most 2 extension headers (Hop-by-Hop, Routing). The verifier will not accept an unbounded loop; a fixed small count is fine.
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
        incr_stat(STAT_V6_EXT_UNSUPPORTED);
        return Ok(xdp_action::XDP_PASS);
    }

    // ROUND8-L4-08: the IPv6 Fragment Extension Header (RFC 2460 §4.5) is present in BOTH first and later fragments, so any packet carrying it lacks a complete L4 header to rewrite.
    if next_hdr == IPPROTO_FRAGMENT {
        incr_stat(STAT_V6_FRAGMENT);
        return Ok(xdp_action::XDP_PASS);
    }

    let (src_port, dst_port, tcp_flags) = match next_hdr {
        IPPROTO_TCP => {
            let tcp = unsafe { ptr_at::<TcpHdr>(ctx, off).ok_or(())? };
            // SAFETY: packed field reads.
            let sp = u16::from_be(unsafe {
                core::ptr::read_unaligned(core::ptr::addr_of!((*tcp).src_port))
            });
            let dp = u16::from_be(unsafe {
                core::ptr::read_unaligned(core::ptr::addr_of!((*tcp).dst_port))
            });
            let flags =
                unsafe { core::ptr::read_unaligned(core::ptr::addr_of!((*tcp).flags)) };
            (sp, dp, flags)
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
            (sp, dp, 0u8)
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

    // ROUND8-L4-02: TCP-state-aware pruning for IPv6 — mirror of the IPv4 path.
    if next_hdr == IPPROTO_TCP && (tcp_flags & TCP_FLAG_RST) != 0 {
        let _ = CONNTRACK_V6.remove(&key);
        incr_stat(STAT_CT_RST_PRUNE);
        incr_stat(STAT_PASS);
        return Ok(xdp_action::XDP_PASS);
    }

    let entry: BackendEntryV6 = match unsafe { CONNTRACK_V6.get(&key) } {
        Some(v) => *v,
        None => {
            // ROUND8-L4-03: mirror of the IPv4 CT-miss flood gate.
            if is_under_flood() {
                incr_stat(STAT_NEW_FLOW_RATE_CAP);
                return Ok(xdp_action::XDP_PASS);
            }
            incr_stat(STAT_PASS);
            return Ok(xdp_action::XDP_PASS);
        }
    };
    incr_stat(STAT_CT_HIT_V6);

    // ROUND8-L4-01: sentinel guard, mirror of the IPv4 path above.
    if entry.backend_ip == [0u8; 16] || entry.backend_port == 0 {
        incr_stat(STAT_BACKEND_UNPOPULATED);
        return Ok(xdp_action::XDP_PASS);
    }

    rewrite_v6(ctx, l3_offset, off, next_hdr, &dst_addr, &entry)?;
    incr_stat(STAT_TX_V6);

    if next_hdr == IPPROTO_TCP
        && (tcp_flags & TCP_FLAG_FIN) != 0
        && (tcp_flags & TCP_FLAG_ACK) != 0
    {
        let _ = CONNTRACK_V6.remove(&key);
        incr_stat(STAT_CT_FIN_PRUNE);
    }

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
    let eth_m = unsafe { ptr_at_mut::<EthHdr>(ctx, 0).ok_or(())? };
    // SAFETY: packed writes on validated pointer.
    unsafe {
        core::ptr::write_unaligned(core::ptr::addr_of_mut!((*eth_m).dst), entry.backend_mac);
        core::ptr::write_unaligned(core::ptr::addr_of_mut!((*eth_m).src), entry.src_mac);
    }

    let ip_m = unsafe { ptr_at_mut::<Ipv6Hdr>(ctx, l3_offset).ok_or(())? };
    // SAFETY: packed write.
    unsafe {
        core::ptr::write_unaligned(core::ptr::addr_of_mut!((*ip_m).dst), entry.backend_ip);
    }

    // L4 checksum update for the 128-bit IPv6 dst in the pseudo-header AND the dst port.
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
                // IPv6 requires a non-zero UDP checksum; we only rewrite if one was already computed.
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
