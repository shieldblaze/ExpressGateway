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

use aya::{
    Ebpf, EbpfError, EbpfLoader,
    maps::{Map, MapError},
    programs::{ProgramError, Xdp, XdpFlags},
};

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
}

/// High-level handle to a loaded BPF object containing an XDP program.
///
/// Constructed by parsing a pre-compiled BPF ELF; nothing is loaded into the
/// kernel until [`XdpLoader::attach`] is called. Map accessors (e.g.
/// [`XdpLoader::take_map`]) allow the userspace control plane to populate
/// `L7_PORTS`, `CONNTRACK`, and `ACL_TRIE` before traffic arrives.
#[derive(Debug)]
pub struct XdpLoader {
    ebpf: Ebpf,
}

impl XdpLoader {
    /// Parse an in-memory BPF ELF. Does not touch the kernel.
    ///
    /// # Errors
    ///
    /// Returns `XdpLoaderError::Load` if the bytes are not a valid BPF object
    /// or if BTF relocation fails.
    pub fn load_from_bytes(elf: &[u8]) -> Result<Self, XdpLoaderError> {
        let ebpf = EbpfLoader::new().load(elf)?;
        Ok(Self { ebpf })
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
    /// through aya's typed map wrappers (e.g. `HashMap<_, FlowKey, BackendEntry>`).
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

/// The compiled BPF ELF, embedded when `scripts/build-xdp.sh` has produced
/// `src/lb_xdp.bin` at build time (detected by `build.rs`). Absent when the
/// toolchain (bpf-linker, LLVM 18 dev headers, rustc nightly + `rust-src`)
/// is not available — in that case this constant simply does not exist.
///
/// The standalone `lb-xdp-ebpf` crate is NOT part of the workspace build
/// (`cargo build --workspace` never compiles it); `scripts/build-xdp.sh`
/// runs the BPF target build separately and installs the ELF next to this
/// source file.
#[cfg(lb_xdp_elf)]
pub const LB_XDP_ELF: &[u8] = include_bytes!("lb_xdp.bin");

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
