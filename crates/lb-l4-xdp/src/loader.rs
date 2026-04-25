//! Aya userspace XDP loader.
//!
//! Parses a BPF ELF object (produced by the standalone `lb-xdp-ebpf` crate
//! under `crates/lb-l4-xdp/ebpf/`) and — on a privileged Linux host —
//! attaches the resulting XDP program to a network interface.
//!
//! Linux-only: this module is compiled only under `cfg(target_os = "linux")`
//! because aya talks directly to the kernel's `bpf(2)` syscall.
//!
//! The loader returns `Result` on every fallible path: no `unwrap`, `expect`,
//! or `panic!` — the crate-wide `#![deny(clippy::unwrap_used, …)]` applies.

use std::io;
use std::net::Ipv4Addr;

use aya::{
    Ebpf, EbpfError, EbpfLoader, Pod,
    maps::{
        HashMap as AyaHashMap, Map, MapData, MapError,
        lpm_trie::{Key as LpmKey, LpmTrie},
    },
    programs::{ProgramError, Xdp, XdpFlags},
};
use aya_obj::{Object, ParseError};

// ---------------------------------------------------------------------------
// Userspace mirrors of the BPF map key/value layouts declared in
// `crates/lb-l4-xdp/ebpf/src/main.rs`. They must stay in lock-step: aya
// compares their byte size against the BPF ELF's declared map sizes on
// accessor construction.
// ---------------------------------------------------------------------------

/// IPv4 flow key — matches `FlowKey` in the ebpf crate byte-for-byte.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FlowKey {
    /// Source IPv4 address (network byte order).
    pub src_addr: u32,
    /// Destination IPv4 address (network byte order).
    pub dst_addr: u32,
    /// Source port (network byte order).
    pub src_port: u16,
    /// Destination port (network byte order).
    pub dst_port: u16,
    /// IP protocol (TCP=6, UDP=17).
    pub protocol: u8,
    /// Padding to keep the key 16 bytes wide for verifier alignment.
    pub pad: [u8; 3],
}

// SAFETY: `FlowKey` is `#[repr(C)]`, `Copy`, and has no padding reads —
// aya's `Pod` is a marker trait requiring `Copy + 'static` layout stability.
unsafe impl Pod for FlowKey {}

/// IPv4 backend entry — matches `BackendEntry` in the ebpf crate.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BackendEntry {
    /// Index into the per-service Maglev table in userspace.
    pub backend_idx: u32,
    /// Reserved flag bits; bit 0 means "rewrite and transmit".
    pub flags: u32,
    /// Backend IPv4 address (network byte order) used by the `XDP_TX` rewrite.
    pub backend_ip: u32,
    /// Backend L4 port (network byte order).
    pub backend_port: u16,
    /// Padding.
    pub pad: u16,
    /// Destination MAC for the rewrite (the backend's).
    pub backend_mac: [u8; 6],
    /// Source MAC for the rewrite (our NIC's).
    pub src_mac: [u8; 6],
}

// SAFETY: `#[repr(C)] + Copy + 'static`; matches ebpf layout exactly.
unsafe impl Pod for BackendEntry {}

/// IPv6 flow key — matches `FlowKeyV6` in the ebpf crate.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct FlowKeyV6 {
    /// Source IPv6 address (network byte order, 16 raw bytes).
    pub src_addr: [u8; 16],
    /// Destination IPv6 address (network byte order, 16 raw bytes).
    pub dst_addr: [u8; 16],
    /// Source port (network byte order).
    pub src_port: u16,
    /// Destination port (network byte order).
    pub dst_port: u16,
    /// IP protocol (TCP=6, UDP=17).
    pub protocol: u8,
    /// Padding to 40 bytes.
    pub pad: [u8; 3],
}

// SAFETY: `#[repr(C)] + Copy + 'static`; matches ebpf layout exactly.
unsafe impl Pod for FlowKeyV6 {}

/// IPv6 backend entry — matches `BackendEntryV6` in the ebpf crate.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BackendEntryV6 {
    /// Index into the userspace Maglev table.
    pub backend_idx: u32,
    /// Reserved flag bits.
    pub flags: u32,
    /// Backend IPv6 address (16 raw bytes).
    pub backend_ip: [u8; 16],
    /// Backend L4 port (network byte order).
    pub backend_port: u16,
    /// Padding.
    pub pad: u16,
    /// Destination MAC for the rewrite (the backend's).
    pub backend_mac: [u8; 6],
    /// Source MAC for the rewrite (our NIC's).
    pub src_mac: [u8; 6],
}

// SAFETY: `#[repr(C)] + Copy + 'static`; matches ebpf layout exactly.
unsafe impl Pod for BackendEntryV6 {}

/// XDP attach mode, mirroring the kernel's `XDP_FLAGS_*` bits.
///
/// `Skb` (generic mode) works on any interface and is the CI/dev default.
/// `Drv` requires NIC driver support. `Hw` requires hardware offload support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XdpMode {
    /// Generic / SKB mode. Universally supported, slower path.
    Skb,
    /// Native driver mode. Requires NIC driver support.
    Drv,
    /// Hardware offload mode. Requires capable NIC hardware.
    Hw,
}

impl XdpMode {
    /// Convert into aya's bitflags type.
    #[must_use]
    pub const fn to_flags(self) -> XdpFlags {
        match self {
            Self::Skb => XdpFlags::SKB_MODE,
            Self::Drv => XdpFlags::DRV_MODE,
            Self::Hw => XdpFlags::HW_MODE,
        }
    }
}

/// Errors surfaced by the aya-backed XDP loader.
#[derive(Debug, thiserror::Error)]
pub enum XdpLoaderError {
    /// Parsing or relocating the BPF ELF failed.
    #[error("ebpf load error: {0}")]
    Load(#[from] EbpfError),

    /// The loaded object did not contain a program with the requested name.
    #[error("program '{0}' not found in ebpf object")]
    ProgramNotFound(String),

    /// The loaded object did not contain a map with the requested name.
    #[error("map '{0}' not found in ebpf object")]
    MapNotFound(&'static str),

    /// A program-level operation (load into kernel, attach, detach) failed.
    #[error("xdp program error: {0}")]
    Program(#[from] ProgramError),

    /// A map-level operation (open, update, delete) failed.
    #[error("bpf map error: {0}")]
    Map(#[from] MapError),

    /// The program entry in the ELF was not an XDP program.
    #[error("program '{0}' is not an XDP program")]
    NotXdp(String),

    /// Raw I/O error (e.g. reading an ELF from disk).
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    /// Object-level ELF parse failed (used by the kernel-free
    /// `parse_object_only` path).
    #[error("ebpf object parse error: {0}")]
    ObjectParse(#[from] ParseError),
}

/// High-level handle to a loaded BPF object containing an XDP program.
///
/// Constructed by parsing a pre-compiled BPF ELF; nothing is loaded into
/// the kernel until [`XdpLoader::attach`] is called. Map accessors
/// ([`XdpLoader::conntrack_map`], [`XdpLoader::conntrack_v6_map`],
/// [`XdpLoader::acl_trie`], [`XdpLoader::take_map`]) allow the userspace
/// control plane to populate `CONNTRACK`, `CONNTRACK_V6`, `ACL_DENY_TRIE`,
/// and `L7_PORTS` before traffic arrives.
#[derive(Debug)]
pub struct XdpLoader {
    ebpf: Ebpf,
}

impl XdpLoader {
    /// Parse an in-memory BPF ELF and have aya create its declared maps
    /// in the kernel. Requires `CAP_BPF` on modern kernels — on
    /// unprivileged CI this will fail at the map-creation step, not at
    /// the parse step.
    ///
    /// For a kernel-free parse (e.g. to inspect program names without
    /// touching the kernel), use [`XdpLoader::program_names`].
    ///
    /// # Errors
    ///
    /// Returns `XdpLoaderError::Load` if the bytes are not a valid BPF
    /// object, BTF relocation fails, or map creation is rejected.
    pub fn load_from_bytes(elf: &[u8]) -> Result<Self, XdpLoaderError> {
        let ebpf = EbpfLoader::new().load(elf)?;
        Ok(Self { ebpf })
    }

    /// Kernel-free ELF inspection: parse the BPF object with aya-obj and
    /// return every program name it declares. Safe to call on
    /// unprivileged CI runners — this never touches the BPF syscall.
    ///
    /// # Errors
    ///
    /// Returns `XdpLoaderError::ObjectParse` if the bytes are not a valid
    /// BPF ELF.
    pub fn program_names(elf: &[u8]) -> Result<Vec<String>, XdpLoaderError> {
        let obj = Object::parse(elf)?;
        Ok(obj.programs.keys().cloned().collect())
    }

    /// Load an XDP program from the object into the kernel.
    ///
    /// Must be called before [`XdpLoader::attach`] for the named program.
    ///
    /// # Errors
    ///
    /// - `XdpLoaderError::ProgramNotFound` if the name is not in the object.
    /// - `XdpLoaderError::NotXdp` if the program is not of XDP type.
    /// - `XdpLoaderError::Program` if the kernel verifier rejects the program.
    pub fn kernel_load(&mut self, prog_name: &str) -> Result<(), XdpLoaderError> {
        let program = self
            .ebpf
            .program_mut(prog_name)
            .ok_or_else(|| XdpLoaderError::ProgramNotFound(prog_name.to_owned()))?;
        let xdp: &mut Xdp = program
            .try_into()
            .map_err(|_| XdpLoaderError::NotXdp(prog_name.to_owned()))?;
        xdp.load()?;
        Ok(())
    }

    /// Attach the kernel-loaded XDP program to a network interface.
    ///
    /// Requires the program to have been loaded via
    /// [`XdpLoader::kernel_load`] first. Requires `CAP_BPF` + `CAP_NET_ADMIN`
    /// on recent kernels; older kernels require `CAP_SYS_ADMIN`.
    ///
    /// # Errors
    ///
    /// Returns `XdpLoaderError::Program` if the interface is unknown or the
    /// kernel refuses the attach (e.g. driver mode unsupported on this NIC).
    pub fn attach(
        &mut self,
        prog_name: &str,
        ifname: &str,
        mode: XdpMode,
    ) -> Result<(), XdpLoaderError> {
        let program = self
            .ebpf
            .program_mut(prog_name)
            .ok_or_else(|| XdpLoaderError::ProgramNotFound(prog_name.to_owned()))?;
        let xdp: &mut Xdp = program
            .try_into()
            .map_err(|_| XdpLoaderError::NotXdp(prog_name.to_owned()))?;
        // attach() returns XdpLinkId; we drop it intentionally — aya keeps the
        // link alive as long as the Xdp handle exists inside self.ebpf.
        let _link_id = xdp.attach(ifname, mode.to_flags())?;
        Ok(())
    }

    /// Take ownership of a BPF map by name so the caller can access it
    /// through aya's typed map wrappers.
    ///
    /// # Errors
    ///
    /// Returns `XdpLoaderError::MapNotFound` if the ELF did not declare a
    /// map by this name.
    pub fn take_map(&mut self, name: &'static str) -> Result<Map, XdpLoaderError> {
        self.ebpf
            .take_map(name)
            .ok_or(XdpLoaderError::MapNotFound(name))
    }

    /// Typed accessor for the IPv4 conntrack map.
    ///
    /// Returns an aya `HashMap` wrapping a mutable borrow of the underlying
    /// `MapData` — the caller can insert, update, or iterate entries.
    ///
    /// # Errors
    ///
    /// - `MapNotFound` when the ELF does not declare `CONNTRACK`.
    /// - `Map` when aya rejects the map (size mismatch between Rust-side
    ///   `FlowKey`/`BackendEntry` and the ELF's declared sizes).
    pub fn conntrack_map(
        &mut self,
    ) -> Result<AyaHashMap<&mut MapData, FlowKey, BackendEntry>, XdpLoaderError> {
        let map = self
            .ebpf
            .map_mut("CONNTRACK")
            .ok_or(XdpLoaderError::MapNotFound("CONNTRACK"))?;
        AyaHashMap::try_from(map).map_err(Into::into)
    }

    /// Typed accessor for the IPv6 conntrack map.
    ///
    /// # Errors
    ///
    /// - `MapNotFound` when the ELF does not declare `CONNTRACK_V6`.
    /// - `Map` when aya rejects the map (size mismatch).
    pub fn conntrack_v6_map(
        &mut self,
    ) -> Result<AyaHashMap<&mut MapData, FlowKeyV6, BackendEntryV6>, XdpLoaderError> {
        let map = self
            .ebpf
            .map_mut("CONNTRACK_V6")
            .ok_or(XdpLoaderError::MapNotFound("CONNTRACK_V6"))?;
        AyaHashMap::try_from(map).map_err(Into::into)
    }

    /// Typed accessor for the IPv4 deny LPM trie (Pillar 4b-2 upgrade
    /// from the Pillar 4a `HashMap<u32, u32>`).
    ///
    /// # Errors
    ///
    /// - `MapNotFound` when the ELF does not declare `ACL_DENY_TRIE`.
    /// - `Map` when aya rejects the map type (e.g. the ELF declares a
    ///   plain hash map).
    pub fn acl_trie(&mut self) -> Result<LpmTrie<&mut MapData, u32, u32>, XdpLoaderError> {
        let map = self
            .ebpf
            .map_mut("ACL_DENY_TRIE")
            .ok_or(XdpLoaderError::MapNotFound("ACL_DENY_TRIE"))?;
        LpmTrie::try_from(map).map_err(Into::into)
    }

    /// Insert a CIDR deny rule into the IPv4 ACL LPM trie. `prefix_len` is
    /// the number of leading bits to match; `ipv4` is the address
    /// (network byte order handled internally). The stored value (`1`)
    /// is an opaque tag — the BPF program only cares about presence.
    ///
    /// # Errors
    ///
    /// Propagates any error from [`XdpLoader::acl_trie`] plus aya-level
    /// `bpf_map_update_elem` failures (full map, permission denied).
    pub fn insert_acl_deny(
        &mut self,
        prefix_len: u8,
        ipv4: Ipv4Addr,
    ) -> Result<(), XdpLoaderError> {
        // aya's examples store IPv4 addresses as u32.to_be() so the BPF
        // side can compare them byte-for-byte against the packet's src_addr
        // (which is already in network byte order). Match that convention.
        let key = LpmKey::<u32>::new(u32::from(prefix_len), u32::from(ipv4).to_be());
        let mut trie = self.acl_trie()?;
        trie.insert(&key, 1u32, 0).map_err(Into::into)
    }

    /// Borrow the underlying `Ebpf` object — escape hatch for callers that
    /// need full aya access (e.g. iterating all maps/programs).
    #[must_use]
    pub const fn ebpf(&self) -> &Ebpf {
        &self.ebpf
    }

    /// Mutably borrow the underlying `Ebpf` object.
    pub const fn ebpf_mut(&mut self) -> &mut Ebpf {
        &mut self.ebpf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Garbage bytes must produce an `XdpLoaderError`, not a panic.
    #[test]
    fn load_garbage_bytes_rejected() {
        let garbage = [0u8; 16];
        let result = XdpLoader::load_from_bytes(&garbage);
        assert!(
            matches!(result, Err(XdpLoaderError::Load(_))),
            "expected Load error for garbage bytes, got {result:?}",
        );
    }

    /// An empty slice is also invalid and must error.
    #[test]
    fn load_empty_bytes_rejected() {
        let empty: [u8; 0] = [];
        let result = XdpLoader::load_from_bytes(&empty);
        assert!(matches!(result, Err(XdpLoaderError::Load(_))));
    }

    /// Each `XdpMode` variant must map to exactly the expected aya flag set.
    /// `XdpFlags` does not implement `PartialEq`, so compare `.bits()`.
    #[test]
    fn xdp_mode_flag_mapping() {
        assert_eq!(XdpMode::Skb.to_flags().bits(), XdpFlags::SKB_MODE.bits());
        assert_eq!(XdpMode::Drv.to_flags().bits(), XdpFlags::DRV_MODE.bits());
        assert_eq!(XdpMode::Hw.to_flags().bits(), XdpFlags::HW_MODE.bits());
        // Distinct bit patterns — cheap sanity to catch accidental collision
        // after refactors.
        assert_ne!(
            XdpMode::Skb.to_flags().bits(),
            XdpMode::Drv.to_flags().bits()
        );
        assert_ne!(
            XdpMode::Drv.to_flags().bits(),
            XdpMode::Hw.to_flags().bits()
        );
    }

    #[test]
    fn xdp_mode_is_copy() {
        let mode = XdpMode::Skb;
        let copy = mode;
        // Both usable after "move" — compiles iff XdpMode: Copy.
        assert_eq!(mode, copy);
    }
}
