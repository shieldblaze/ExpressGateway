//! BPF map definitions shared between eBPF programs and userspace.
//!
//! All structs use `#[repr(C)]` for stable ABI layout matching what the
//! kernel-side BPF programs expect. Fields that cross the kernel/userspace
//! boundary must remain `#[repr(C)]` -- never `#[repr(Rust)]`.
//!
//! # Map types used
//!
//! - `BPF_MAP_TYPE_HASH` -- TCP connection tracking (`TcpConnKey` -> `TcpConnValue`)
//! - `BPF_MAP_TYPE_HASH` -- UDP session tracking (`UdpSessionKey` -> `UdpSessionValue`)
//! - `BPF_MAP_TYPE_ARRAY` -- Backend list (`u32` index -> `BackendEntry`)
//! - `BPF_MAP_TYPE_LPM_TRIE` -- ACL rules (`AclKey` -> `AclValue`)
//! - `BPF_MAP_TYPE_PERCPU_ARRAY` -- Statistics (`u32` index -> `XdpStats`)

/// TCP connection table key (network byte order for IP fields).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct TcpConnKey {
    pub src_ip: u32,
    pub src_port: u16,
    pub dst_ip: u32,
    pub dst_port: u16,
}

/// TCP connection table value.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct TcpConnValue {
    pub backend_ip: u32,
    pub backend_port: u16,
    /// Connection state: 0 = NEW, 1 = ESTABLISHED, 2 = CLOSING.
    pub state: u8,
}

/// TCP connection states.
pub mod tcp_state {
    pub const NEW: u8 = 0;
    pub const ESTABLISHED: u8 = 1;
    pub const CLOSING: u8 = 2;
}

/// UDP session table key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct UdpSessionKey {
    pub src_ip: u32,
    pub src_port: u16,
}

/// UDP session table value.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct UdpSessionValue {
    pub backend_ip: u32,
    pub backend_port: u16,
    /// Nanoseconds since boot (from `bpf_ktime_get_ns`).
    pub last_seen: u64,
}

/// Backend entry in the backend array map.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct BackendEntry {
    pub ip: u32,
    pub port: u16,
    pub weight: u16,
    /// Backend state: 0 = OFFLINE, 1 = ONLINE.
    pub state: u8,
}

/// Backend states.
pub mod backend_state {
    pub const OFFLINE: u8 = 0;
    pub const ONLINE: u8 = 1;
}

/// ACL trie key (longest-prefix-match).
///
/// Layout matches `struct bpf_lpm_trie_key` plus the data field.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct AclKey {
    /// Number of significant prefix bits.
    pub prefix_len: u32,
    /// IPv4 (first 4 bytes) or IPv6 (all 16 bytes).
    pub ip: [u8; 16],
}

impl AclKey {
    /// Create an ACL key from an IPv4 address and prefix length.
    #[inline]
    pub fn from_ipv4(addr: [u8; 4], prefix_len: u32) -> Self {
        let mut ip = [0u8; 16];
        ip[..4].copy_from_slice(&addr);
        Self { prefix_len, ip }
    }

    /// Create an ACL key from an IPv6 address and prefix length.
    #[inline]
    pub fn from_ipv6(addr: [u8; 16], prefix_len: u32) -> Self {
        Self {
            prefix_len,
            ip: addr,
        }
    }
}

/// ACL trie value.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct AclValue {
    /// Action: 0 = DENY, 1 = ALLOW.
    pub action: u8,
    /// Associated rate-limit rule ID (0 = none).
    pub rate_limit_id: u32,
}

/// ACL actions.
pub mod acl_action {
    pub const DENY: u8 = 0;
    pub const ALLOW: u8 = 1;
}

/// Per-CPU XDP statistics counters.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct XdpStats {
    pub packets_tx: u64,
    pub packets_redirect: u64,
    pub packets_pass: u64,
    pub packets_drop: u64,
    pub bytes_tx: u64,
    pub bytes_redirect: u64,
}

impl XdpStats {
    /// Merge per-CPU stats by summing all fields.
    #[inline]
    pub fn merge(&mut self, other: &XdpStats) {
        self.packets_tx += other.packets_tx;
        self.packets_redirect += other.packets_redirect;
        self.packets_pass += other.packets_pass;
        self.packets_drop += other.packets_drop;
        self.bytes_tx += other.bytes_tx;
        self.bytes_redirect += other.bytes_redirect;
    }

    /// Total packets processed across all actions.
    #[inline]
    pub fn total_packets(&self) -> u64 {
        self.packets_tx + self.packets_redirect + self.packets_pass + self.packets_drop
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem;

    #[test]
    fn tcp_conn_key_size() {
        let size = mem::size_of::<TcpConnKey>();
        assert!(size >= 12, "TcpConnKey too small: {size}");
        assert_eq!(mem::align_of::<TcpConnKey>(), mem::align_of::<u32>());
    }

    #[test]
    fn tcp_conn_value_size() {
        let size = mem::size_of::<TcpConnValue>();
        assert!(size >= 7, "TcpConnValue too small: {size}");
    }

    #[test]
    fn udp_session_key_size() {
        let size = mem::size_of::<UdpSessionKey>();
        assert!(size >= 6, "UdpSessionKey too small: {size}");
    }

    #[test]
    fn udp_session_value_size() {
        let size = mem::size_of::<UdpSessionValue>();
        assert!(size >= 14, "UdpSessionValue too small: {size}");
    }

    #[test]
    fn backend_entry_size() {
        let size = mem::size_of::<BackendEntry>();
        assert!(size >= 7, "BackendEntry too small: {size}");
    }

    #[test]
    fn acl_key_size() {
        let size = mem::size_of::<AclKey>();
        assert_eq!(size, 20);
    }

    #[test]
    fn acl_value_size() {
        let size = mem::size_of::<AclValue>();
        assert!(size >= 5, "AclValue too small: {size}");
    }

    #[test]
    fn xdp_stats_size() {
        assert_eq!(mem::size_of::<XdpStats>(), 48);
    }

    #[test]
    fn xdp_stats_default() {
        let stats = XdpStats::default();
        assert_eq!(stats.packets_tx, 0);
        assert_eq!(stats.packets_redirect, 0);
        assert_eq!(stats.packets_pass, 0);
        assert_eq!(stats.packets_drop, 0);
        assert_eq!(stats.bytes_tx, 0);
        assert_eq!(stats.bytes_redirect, 0);
    }

    #[test]
    fn xdp_stats_merge() {
        let mut a = XdpStats {
            packets_tx: 10,
            packets_redirect: 5,
            packets_pass: 3,
            packets_drop: 1,
            bytes_tx: 1000,
            bytes_redirect: 500,
        };
        let b = XdpStats {
            packets_tx: 20,
            packets_redirect: 10,
            packets_pass: 7,
            packets_drop: 2,
            bytes_tx: 2000,
            bytes_redirect: 1000,
        };
        a.merge(&b);
        assert_eq!(a.packets_tx, 30);
        assert_eq!(a.packets_redirect, 15);
        assert_eq!(a.packets_pass, 10);
        assert_eq!(a.packets_drop, 3);
        assert_eq!(a.total_packets(), 58);
    }

    #[test]
    fn tcp_conn_key_hash_eq() {
        let a = TcpConnKey {
            src_ip: 0x0100007f,
            src_port: 8080,
            dst_ip: 0x0200007f,
            dst_port: 80,
        };
        let b = a;
        assert_eq!(a, b);

        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(a);
        assert!(set.contains(&b));
    }

    #[test]
    fn acl_key_from_ipv4() {
        let key = AclKey::from_ipv4([10, 0, 0, 0], 8);
        assert_eq!(key.prefix_len, 8);
        assert_eq!(&key.ip[..4], &[10, 0, 0, 0]);
        assert_eq!(&key.ip[4..], &[0u8; 12]);
    }

    #[test]
    fn acl_key_from_ipv6() {
        let addr = [0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        let key = AclKey::from_ipv6(addr, 64);
        assert_eq!(key.prefix_len, 64);
        assert_eq!(key.ip, addr);
    }
}
