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

// SEC-2-12: belt-and-suspenders ELF license check. We parse the ELF
// *again* with `object` (a kernel-free, no_std-friendly parser) so the
// assertion runs before aya's `EbpfLoader::load` ever touches the
// BPF syscall. `object::Object` and `aya_obj::Object` share a name —
// alias them to avoid the import collision.
use object::{File as ObjectFile, Object as ObjectTrait, ObjectSection as ObjectSectionTrait};

// ---------------------------------------------------------------------------
// EBPF-2-05: stable map pin names. These MUST match the `#[map(name =
// "...")]` strings in `crates/lb-l4-xdp/ebpf/src/main.rs`. Pin files
// are created at `<pin_dir>/<NAME>` by aya when
// `EbpfLoader::map_pin_path` is set; downstream observability
// tooling (bpftool, cilium-cli) uses the same string.
// ---------------------------------------------------------------------------

/// Pin filename of the IPv4 conntrack map. Matches
/// `#[map(name = "conntrack")]` in `ebpf/src/main.rs`.
pub const CONNTRACK_PIN_NAME: &str = "conntrack";

/// Pin filename of the IPv6 conntrack map.
pub const CONNTRACK_V6_PIN_NAME: &str = "conntrack_v6";

/// Pin filename of the L7 ports table (config-managed; not flood-pressured).
pub const L7_PORTS_PIN_NAME: &str = "l7_ports";

/// Pin filename of the IPv4 deny LPM trie.
pub const ACL_DENY_TRIE_PIN_NAME: &str = "acl_deny_trie";

/// Pin filename of the per-CPU stats array (EBPF-2-08 exposes the
/// counter slots via `stats_export.rs`).
pub const STATS_PIN_NAME: &str = "stats";

/// Default bpffs root for the production deployment. The directory
/// itself must be created with `0750` ownership of the LB uid:gid
/// before the loader runs — see `crates/lb/src/xdp.rs` and the
/// systemd unit. Tests override via `EG_BPFFS_ROOT` env var.
pub const DEFAULT_PIN_DIR: &str = "/sys/fs/bpf/expressgateway";

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
    ///
    /// CODE-2-07: kept `pub` for backwards-compatibility with existing
    /// struct-literal construction sites (`lib.rs`, `sim.rs`). New code
    /// SHOULD prefer [`FlowKey::new`] which guarantees the padding is
    /// zero-initialised — avoiding the (currently safe but fragile)
    /// risk of a future contributor reaching for
    /// `MaybeUninit::uninit().assume_init()`.
    pub pad: [u8; 3],
}

// SAFETY: `FlowKey` is `#[repr(C)]`, `Copy`, and has no padding reads —
// aya's `Pod` is a marker trait requiring `Copy + 'static` layout stability.
unsafe impl Pod for FlowKey {}

impl FlowKey {
    /// Construct a [`FlowKey`] with explicit zero-initialised padding.
    ///
    /// CODE-2-07: the in-tree struct-literal sites are all correct
    /// today, but the only way to keep that property under refactor is
    /// to funnel callers through a constructor that owns the
    /// zero-init contract. The `pad` bytes are set to `[0u8; 3]`
    /// unconditionally — callers may not override them.
    ///
    /// Sizes are documented at [`FLOWKEY_SIZE`].
    #[must_use]
    pub const fn new(
        src_addr: u32,
        src_port: u16,
        dst_addr: u32,
        dst_port: u16,
        protocol: u8,
    ) -> Self {
        Self {
            src_addr,
            dst_addr,
            src_port,
            dst_port,
            protocol,
            pad: [0u8; 3],
        }
    }
}

/// IPv4 backend entry — matches `BackendEntry` in the ebpf crate.
///
/// ROUND8-L4-07: the legacy `flags: u32` field was a
/// documented-but-unused field (BPF program never read it). Dropped
/// to save 4 B/entry × 1M entries = 4 MB BPF map memory and to
/// remove the doc-vs-code drift the audit caught. Callers that were
/// constructing with `flags = 0` need to drop the argument from
/// `BackendEntry::new` / `try_new`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BackendEntry {
    /// Index into the per-service Maglev table in userspace.
    pub backend_idx: u32,
    /// Backend IPv4 address (network byte order) used by the `XDP_TX` rewrite.
    pub backend_ip: u32,
    /// Backend L4 port (network byte order).
    pub backend_port: u16,
    /// Padding. CODE-2-07: prefer [`BackendEntry::new`] which
    /// zero-initialises this field.
    pub pad: u16,
    /// Destination MAC for the rewrite (the backend's).
    pub backend_mac: [u8; 6],
    /// Source MAC for the rewrite (our NIC's).
    pub src_mac: [u8; 6],
}

// SAFETY: `#[repr(C)] + Copy + 'static`; matches ebpf layout exactly.
unsafe impl Pod for BackendEntry {}

impl BackendEntry {
    /// Construct a [`BackendEntry`] with zero-initialised padding.
    ///
    /// CODE-2-07: see [`FlowKey::new`] for the rationale.
    ///
    /// **ROUND8-L4-01 caveat**: this constructor is infallible for
    /// back-compat; it does NOT reject zero IP / zero port. New
    /// callers SHOULD use [`BackendEntry::try_new`] which returns
    /// `Err(XdpLoaderError::BackendUnpopulated)` on the sentinel
    /// shapes that cause silent traffic drops. The eBPF data plane
    /// also enforces a runtime sentinel guard — a CT entry with
    /// `backend_ip == 0` returns `XDP_PASS` plus the
    /// `backend_unpopulated` stat slot increment.
    #[must_use]
    pub const fn new(
        backend_idx: u32,
        backend_ip: u32,
        backend_port: u16,
        backend_mac: [u8; 6],
        src_mac: [u8; 6],
    ) -> Self {
        Self {
            backend_idx,
            backend_ip,
            backend_port,
            pad: 0,
            backend_mac,
            src_mac,
        }
    }

    /// ROUND8-L4-01: fallible constructor — rejects the
    /// `backend_ip == 0` and `backend_port == 0` sentinel shapes.
    /// These shapes are the Katran-class silent-drop vector: a
    /// conntrack entry with a zero backend results in `XDP_TX` to
    /// `0.0.0.0:0` which the kernel drops without telemetry. The
    /// eBPF program also enforces the guard at runtime, but this
    /// constructor is the upstream admission gate.
    ///
    /// # Errors
    ///
    /// Returns [`XdpLoaderError::BackendUnpopulated`] when
    /// `backend_ip == 0` (Katran lesson 10 specifically calls out
    /// reserving the zero IP) or `backend_port == 0` (kernel never
    /// generates a flow to port 0).
    pub fn try_new(
        backend_idx: u32,
        backend_ip: u32,
        backend_port: u16,
        backend_mac: [u8; 6],
        src_mac: [u8; 6],
    ) -> Result<Self, XdpLoaderError> {
        if backend_ip == 0 {
            return Err(XdpLoaderError::BackendUnpopulated {
                reason: "backend_ip is 0.0.0.0 (Katran-class silent-drop sentinel)",
            });
        }
        if backend_port == 0 {
            return Err(XdpLoaderError::BackendUnpopulated {
                reason: "backend_port is 0",
            });
        }
        Ok(Self::new(
            backend_idx,
            backend_ip,
            backend_port,
            backend_mac,
            src_mac,
        ))
    }
}

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
    /// Padding to 40 bytes. CODE-2-07: prefer [`FlowKeyV6::new`].
    pub pad: [u8; 3],
}

// SAFETY: `#[repr(C)] + Copy + 'static`; matches ebpf layout exactly.
unsafe impl Pod for FlowKeyV6 {}

impl FlowKeyV6 {
    /// Construct a [`FlowKeyV6`] with zero-initialised padding.
    #[must_use]
    pub const fn new(
        src_addr: [u8; 16],
        src_port: u16,
        dst_addr: [u8; 16],
        dst_port: u16,
        protocol: u8,
    ) -> Self {
        Self {
            src_addr,
            dst_addr,
            src_port,
            dst_port,
            protocol,
            pad: [0u8; 3],
        }
    }
}

/// IPv6 backend entry — matches `BackendEntryV6` in the ebpf crate.
///
/// ROUND8-L4-07: dropped the legacy `flags: u32`. See [`BackendEntry`].
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BackendEntryV6 {
    /// Index into the userspace Maglev table.
    pub backend_idx: u32,
    /// Backend IPv6 address (16 raw bytes).
    pub backend_ip: [u8; 16],
    /// Backend L4 port (network byte order).
    pub backend_port: u16,
    /// Padding. CODE-2-07: prefer [`BackendEntryV6::new`].
    pub pad: u16,
    /// Destination MAC for the rewrite (the backend's).
    pub backend_mac: [u8; 6],
    /// Source MAC for the rewrite (our NIC's).
    pub src_mac: [u8; 6],
}

// SAFETY: `#[repr(C)] + Copy + 'static`; matches ebpf layout exactly.
unsafe impl Pod for BackendEntryV6 {}

impl BackendEntryV6 {
    /// Construct a [`BackendEntryV6`] with zero-initialised padding.
    ///
    /// **ROUND8-L4-01 caveat**: see [`BackendEntry::new`] — prefer
    /// [`BackendEntryV6::try_new`] for new callers; this constructor
    /// is infallible for back-compat and does not reject the zero
    /// sentinel shapes.
    #[must_use]
    pub const fn new(
        backend_idx: u32,
        backend_ip: [u8; 16],
        backend_port: u16,
        backend_mac: [u8; 6],
        src_mac: [u8; 6],
    ) -> Self {
        Self {
            backend_idx,
            backend_ip,
            backend_port,
            pad: 0,
            backend_mac,
            src_mac,
        }
    }

    /// ROUND8-L4-01: fallible constructor for IPv6. Rejects
    /// `backend_ip == [0; 16]` (the IPv6 unspecified address) and
    /// `backend_port == 0`. See [`BackendEntry::try_new`] for the
    /// rationale and the eBPF-side mirror guard.
    ///
    /// # Errors
    ///
    /// Returns [`XdpLoaderError::BackendUnpopulated`].
    pub fn try_new(
        backend_idx: u32,
        backend_ip: [u8; 16],
        backend_port: u16,
        backend_mac: [u8; 6],
        src_mac: [u8; 6],
    ) -> Result<Self, XdpLoaderError> {
        if backend_ip == [0u8; 16] {
            return Err(XdpLoaderError::BackendUnpopulated {
                reason: "backend_ip is :: (IPv6 unspecified)",
            });
        }
        if backend_port == 0 {
            return Err(XdpLoaderError::BackendUnpopulated {
                reason: "backend_port is 0",
            });
        }
        Ok(Self::new(
            backend_idx,
            backend_ip,
            backend_port,
            backend_mac,
            src_mac,
        ))
    }
}

// CODE-2-07: byte-size assertions matching the BPF-side struct layouts.
// These compile-time checks fail the build if either side's layout
// drifts (e.g. a `pad` byte is dropped or a field width changes).
//
// FlowKey:        4 + 4 + 2 + 2 + 1 + 3 = 16
// FlowKeyV6:      16 + 16 + 2 + 2 + 1 + 3 = 40
// BackendEntry:   4 + 4 + 2 + 2 + 6 + 6 = 24  (ROUND8-L4-07: dropped 4 B flags)
// BackendEntryV6: 4 + 16 + 2 + 2 + 6 + 6 = 36 (ROUND8-L4-07: dropped 4 B flags)

/// Expected wire size of [`FlowKey`] (matches BPF-side struct).
pub const FLOWKEY_SIZE: usize = 16;
/// Expected wire size of [`FlowKeyV6`] (matches BPF-side struct).
pub const FLOWKEY_V6_SIZE: usize = 40;
/// Expected wire size of [`BackendEntry`] (matches BPF-side struct).
///
/// ROUND8-L4-07: down from 28 → 24 after dropping the
/// documented-but-unused `flags: u32` field. Saves 4 B/entry; at
/// CONNTRACK's 1M max_entries that is 4 MB BPF map memory.
pub const BACKEND_ENTRY_SIZE: usize = 24;
/// Expected wire size of [`BackendEntryV6`] (matches BPF-side struct).
///
/// ROUND8-L4-07: down from 40 → 36 (same rationale as
/// [`BACKEND_ENTRY_SIZE`]; CONNTRACK_V6 max_entries is 512k →
/// 2 MB saved).
pub const BACKEND_ENTRY_V6_SIZE: usize = 36;

const _: () = assert!(core::mem::size_of::<FlowKey>() == FLOWKEY_SIZE);
const _: () = assert!(core::mem::size_of::<FlowKeyV6>() == FLOWKEY_V6_SIZE);
const _: () = assert!(core::mem::size_of::<BackendEntry>() == BACKEND_ENTRY_SIZE);
const _: () = assert!(core::mem::size_of::<BackendEntryV6>() == BACKEND_ENTRY_V6_SIZE);

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

    /// EBPF-2-04: translate to the telemetry label exposed via
    /// [`crate::stats_export`]. Kept symmetric with `XdpFlags` so a
    /// future kernel mode added to aya gets a compile error here.
    #[must_use]
    pub const fn to_label(self) -> crate::stats_export::AttachModeLabel {
        match self {
            Self::Skb => crate::stats_export::AttachModeLabel::Skb,
            Self::Drv => crate::stats_export::AttachModeLabel::Drv,
            Self::Hw => crate::stats_export::AttachModeLabel::Hw,
        }
    }
}

/// EBPF-2-04: classify a `ProgramError` as "mode unsupported by this
/// NIC" — the only errnos that trigger ladder fall-through. Any other
/// error means a real bug (verifier reject, bad ifname, ...) and the
/// ladder MUST NOT swallow it.
///
/// Errno values are kernel-stable on Linux: EOPNOTSUPP=95, EINVAL=22.
/// Coded as literals to avoid a `libc` direct dependency.
fn is_unsupported_mode(e: &ProgramError) -> bool {
    const EINVAL: i32 = 22;
    const EOPNOTSUPP: i32 = 95;
    if let ProgramError::SyscallError(sc) = e {
        let raw = sc.io_error.raw_os_error();
        return matches!(raw, Some(EINVAL) | Some(EOPNOTSUPP));
    }
    false
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

    /// EBPF-2-04: the attach-mode ladder ran out of modes to try.
    /// Carries the original errno-bearing error of the last attempt so
    /// the operator can tell what the NIC actually rejected.
    #[error("all xdp attach modes exhausted; last error: {0}")]
    AllAttachModesExhausted(String),

    /// EBPF-2-08: installing the STATS per-CPU handle into the
    /// [`crate::stats_export`] module failed (already installed, or
    /// the map type didn't match `u64`).
    #[error("stats export install failed: {0}")]
    StatsExport(String),

    /// SEC-2-12: the loaded ELF's `license` section is missing or
    /// does not contain the byte-for-byte string `"GPL\0"`. Belt-and-
    /// suspenders against an accidental rebuild that strips the
    /// `#[link_section = "license"]` static (e.g. a different
    /// toolchain or a typo in the ebpf crate's root).
    ///
    /// This is a fail-fast: most BPF helpers used by lb-xdp are
    /// `gpl_only=true` and the kernel verifier rejects the program
    /// otherwise. Catching it here gives the operator a clear error
    /// at startup instead of `EACCES` at `bpf(BPF_PROG_LOAD)`.
    #[error("bpf elf license check failed: {0}")]
    LicenseInvalid(String),

    /// ROUND8-L4-06: caller passed an out-of-range CIDR prefix length
    /// to [`XdpLoader::insert_acl_deny`]. The accepted range is
    /// `1..=32` for IPv4 (a `/0` deny is the "block everything"
    /// footgun the finding documents; `/33`+ is structurally
    /// nonsensical). The simulator (`crate::sim`) already documents
    /// that `/0` matches every packet — this guard makes the public
    /// API refuse to install such an entry.
    #[error("invalid IPv4 ACL prefix length: got {0}, must be in 1..=32")]
    InvalidAclPrefixV4(u8),

    /// ROUND8-L4-01: caller tried to construct a `BackendEntry` /
    /// `BackendEntryV6` with `backend_ip == 0` or `backend_port == 0`.
    /// Katran lesson 10: a zero-IP backend in the conntrack table
    /// causes silent `XDP_TX` to `0.0.0.0:0` — the kernel drops it,
    /// flow loss is invisible. The eBPF data plane also enforces a
    /// runtime sentinel guard (returns `XDP_PASS` plus a
    /// `backend_unpopulated` stat increment); this error is the
    /// construction-time admission gate.
    #[error("backend entry unpopulated: {reason}")]
    BackendUnpopulated {
        /// Operator-facing description (which field was zero).
        reason: &'static str,
    },

    /// ROUND8-L4-11: the `pin_dir` argument to
    /// [`XdpLoader::load_from_bytes_pinned`] resolves to a directory
    /// that is NOT backed by bpffs. Pinning into a regular tmpfs
    /// would cause aya to deep-fail with an opaque EINVAL; this
    /// fail-fast surfaces the actionable remediation (mount bpffs).
    #[error("pin path {path:?} is not bpffs (found magic 0x{found_magic:x}); {hint}")]
    PinPathNotBpffs {
        /// The bad path the loader was asked to use.
        path: std::path::PathBuf,
        /// `statfs.f_type` value the kernel returned for the path.
        found_magic: i64,
        /// Operator-actionable next step (mount command).
        hint: String,
    },

    /// ROUND8-L4-11: the `statfs(2)` call on the pin directory
    /// itself failed (path missing, permission denied, ...).
    #[error("statfs on pin path {path:?} failed: {source}")]
    PinPathStatFailed {
        /// The path that could not be stat'd.
        path: std::path::PathBuf,
        /// Underlying I/O / errno-bearing error.
        #[source]
        source: io::Error,
    },

    /// ROUND8-L4-12: `XdpLoader::attach_replacing` found a foreign
    /// XDP program already attached to the interface. The replace
    /// would have clobbered an unrelated tool — fail loudly so the
    /// operator can resolve the conflict (`bpftool net detach`).
    #[error("foreign XDP program attached: prog_id={0}; refusing to attach")]
    ForeignProgramAttached(u32),

    /// ROUND8-L4-12: `XdpLoader::attach_replacing` /
    /// `detach_verifying` expected an XDP program to be attached but
    /// the kernel reports none. For `detach_verifying` this is the
    /// idempotent / already-detached case the drain coordinator may
    /// log INFO and continue; for `attach_replacing` it's a hard
    /// error (we cannot replace nothing).
    #[error("no XDP program attached to {0}")]
    NoProgramAttached(String),

    /// ROUND8-L4-12: `detach_verifying` returned successfully but
    /// the post-detach kernel query still shows a program attached.
    /// Should be impossible on a healthy kernel; treat as a bug
    /// signal and surface immediately.
    #[error("detach left a program attached on {iface}: prog_id={prog_id:?}")]
    DetachLeftProgramAttached {
        /// Interface name.
        iface: String,
        /// Surviving prog_id (if any).
        prog_id: Option<u32>,
    },
}

/// SEC-2-12: required value of the ELF `license` section.
///
/// Kernel-side `bpf_attr.license` is a NUL-terminated C string and
/// must compare equal to `"GPL"` (or another GPL-compatible string).
/// We compile the eBPF crate with `#[link_section = "license"]
/// #[used] pub static LICENSE: [u8; 4] = *b"GPL\0";`, so the section
/// payload is exactly four bytes including the trailing NUL.
const EXPECTED_LICENSE: &[u8] = b"GPL\0";

/// SEC-2-12: parse the ELF and assert its `license` section equals
/// `"GPL\0"`. Returns [`XdpLoaderError::LicenseInvalid`] on any
/// mismatch (missing section, wrong contents, unreadable data).
///
/// Pulled out into a free function so unit tests can synthesise an
/// ELF without the section and prove the assertion trips.
fn assert_license_is_gpl(elf: &[u8]) -> Result<(), XdpLoaderError> {
    let parsed = ObjectFile::parse(elf).map_err(|e| {
        XdpLoaderError::LicenseInvalid(format!("could not parse ELF for license check: {e}"))
    })?;
    let section = parsed.section_by_name("license").ok_or_else(|| {
        XdpLoaderError::LicenseInvalid(
            "ELF is missing the `license` section — rebuild the lb-xdp ebpf crate \
             with #[link_section = \"license\"] (see EBPF-2-01)"
                .to_owned(),
        )
    })?;
    let data = section.data().map_err(|e| {
        XdpLoaderError::LicenseInvalid(format!("could not read `license` section: {e}"))
    })?;
    if data != EXPECTED_LICENSE {
        return Err(XdpLoaderError::LicenseInvalid(format!(
            "expected {EXPECTED_LICENSE:?}, got {data:?} — the eBPF crate's \
             LICENSE static may have been overwritten or stripped by a custom toolchain",
        )));
    }
    Ok(())
}

/// EBPF-2-04: outcome of [`XdpLoader::attach_with_fallback`].
///
/// `mode` is the mode the kernel accepted; `attempts` is the number of
/// ladder steps tried (1 = first try succeeded). Surfaced into the
/// `xdp_attach_attempts_total` counter by `crates/lb/src/xdp.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttachOutcome {
    /// The mode the kernel accepted.
    pub mode: XdpMode,
    /// How many ladder steps were tried (>=1).
    pub attempts: u8,
}

/// EBPF-2-04: operator-facing knob mirroring
/// [`lb_config::XdpModeChoice`]. Kept here so this crate can stay
/// non-circular with `lb-config` if/when ownership flips.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum XdpModeChoice {
    /// Drv → Skb fallback ladder.
    #[default]
    Auto,
    /// Drv only. Loud-fail on unsupported.
    Native,
    /// Skb only (today's behaviour pre-EBPF-2-04).
    Skb,
    /// Hw only. Loud-fail on unsupported.
    Hw,
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
        Self::load_from_bytes_pinned(elf, None)
    }

    /// EBPF-2-05: load with an optional `map_pin_path` so the maps
    /// survive a process restart.
    ///
    /// When `pin_path = Some(dir)`, aya pins every map under
    /// `dir/<pin-name>` (the lowercase names declared via
    /// `#[map(name = "...")]` in the eBPF source — see
    /// [`CONNTRACK_PIN_NAME`] et al.). Aya transparently reuses an
    /// existing pin when the kernel-side map's `map_type`, `key_size`
    /// and `value_size` match the ELF declaration; on size mismatch
    /// it returns [`MapError::InvalidPin`] which surfaces as
    /// [`XdpLoaderError::Map`]. Callers that want to recover from a
    /// schema-mismatch must unlink the stale pin files and retry.
    ///
    /// Caller is responsible for ensuring the directory exists with
    /// the correct mode/owner (`0750` owned by the LB uid:gid is the
    /// recommended posture; see DEPLOYMENT.md / RUNBOOK.md).
    ///
    /// ROUND8-L4-11: the mount-type is verified at runtime via
    /// [`crate::bpffs::assert_bpffs`] — passing a path that is not
    /// backed by `BPF_FS_MAGIC` returns
    /// [`XdpLoaderError::PinPathNotBpffs`] with an operator-actionable
    /// remediation hint, instead of a deep-aya `EbpfError::Map(InvalidPin)`
    /// trail. The bpffs mount itself is established by the systemd
    /// unit (`packaging/expressgateway.service`,
    /// `RequiresMountsFor=/sys/fs/bpf`) per OPS-07.
    ///
    /// # Errors
    ///
    /// - [`XdpLoaderError::PinPathStatFailed`] if `statfs(2)` on the
    ///   pin path fails (missing dir, EACCES, ...).
    /// - [`XdpLoaderError::PinPathNotBpffs`] if `statfs` reports a
    ///   filesystem magic other than `BPF_FS_MAGIC` on the path.
    /// - [`XdpLoaderError::LicenseInvalid`] if the ELF's `license`
    ///   section is missing or wrong (SEC-2-12).
    /// - [`XdpLoaderError::Load`] if the bytes are not a valid BPF
    ///   object, BTF relocation fails, or map creation is rejected.
    pub fn load_from_bytes_pinned(
        elf: &[u8],
        pin_path: Option<&std::path::Path>,
    ) -> Result<Self, XdpLoaderError> {
        // SEC-2-12: belt-and-suspenders. Verify the ELF declares
        // `license = "GPL\0"` BEFORE handing it to aya / the kernel.
        // The kernel verifier would reject most programs on a wrong
        // license anyway, but doing it here gives the operator a
        // clear "rebuild the ebpf crate" message at startup instead
        // of `EACCES` deep inside `bpf(BPF_PROG_LOAD)`.
        assert_license_is_gpl(elf)?;
        let mut loader = EbpfLoader::new();
        if let Some(p) = pin_path {
            // ROUND8-L4-11: fail-fast on a non-bpffs pin directory.
            // Runs BEFORE `loader.map_pin_path(p)` so the operator
            // sees the typed error instead of the deep-aya EINVAL
            // they used to see when aya tried to BPF_OBJ_GET against
            // a regular tmpfs path.
            crate::bpffs::assert_bpffs(p)?;
            loader.map_pin_path(p);
        }
        let ebpf = loader.load(elf)?;
        Ok(Self { ebpf })
    }

    /// EBPF-2-08: hand the STATS per-CPU array to
    /// [`crate::stats_export`] so rel's Prom scraper can read it.
    /// Idempotent at the first call; a second call returns
    /// [`XdpLoaderError::Map`] with "STATS handle already installed".
    ///
    /// The map is taken (not borrowed) so the loader can't double-
    /// install it on a subsequent call — single ownership matches
    /// the once-per-process invariant `crates/lb/src/xdp.rs` relies
    /// on.
    ///
    /// # Errors
    ///
    /// - [`XdpLoaderError::MapNotFound`]: the ELF did not declare a
    ///   `stats` map.
    /// - [`XdpLoaderError::Map`]: aya rejected the typed conversion
    ///   (size mismatch between Rust-side `u64` and the ELF's
    ///   declared value size) OR the handle was already installed.
    pub fn install_stats_export(&mut self) -> Result<(), XdpLoaderError> {
        let map = self.take_map(STATS_PIN_NAME)?;
        crate::stats_export::install_stats_handle(map)
            .map_err(|e| XdpLoaderError::StatsExport(e.to_string()))
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

    /// EBPF-2-04: probe ladder for XDP attach.
    ///
    /// Translates an operator-facing [`XdpModeChoice`] into a sequence
    /// of [`XdpMode`] attempts. Falls back from Drv to Skb only when
    /// the kernel responds with `EOPNOTSUPP` or `EINVAL` (the two
    /// errnos that mean "this NIC doesn't support this mode"); any
    /// other error short-circuits to surface the real failure. The
    /// `Native` and `Hw` choices intentionally skip the ladder so an
    /// operator who asked for Native gets a loud startup failure
    /// rather than a silent 10-50x throughput regression to SKB.
    ///
    /// Side-effect: on success, calls
    /// [`crate::stats_export::record_attach_mode`] so rel's Prom
    /// scrape sees the chosen mode without re-querying the kernel.
    ///
    /// # Errors
    ///
    /// - [`XdpLoaderError::ProgramNotFound`] / [`XdpLoaderError::NotXdp`]:
    ///   propagate from the underlying program lookup.
    /// - [`XdpLoaderError::Program`]: a non-`EOPNOTSUPP`/`EINVAL`
    ///   attach failure — surfaced as-is.
    /// - [`XdpLoaderError::AllAttachModesExhausted`]: every mode in
    ///   the ladder returned `EOPNOTSUPP`/`EINVAL`.
    pub fn attach_with_fallback(
        &mut self,
        prog_name: &str,
        ifname: &str,
        requested: XdpModeChoice,
    ) -> Result<AttachOutcome, XdpLoaderError> {
        // Ladder definitions live here, NOT in the caller — so the
        // policy is single-sourced and the test in
        // `tests/xdp_attach_mode.rs` covers every branch.
        let order: &[XdpMode] = match requested {
            XdpModeChoice::Auto => &[XdpMode::Drv, XdpMode::Skb],
            XdpModeChoice::Native => &[XdpMode::Drv],
            XdpModeChoice::Skb => &[XdpMode::Skb],
            XdpModeChoice::Hw => &[XdpMode::Hw],
        };
        let program = self
            .ebpf
            .program_mut(prog_name)
            .ok_or_else(|| XdpLoaderError::ProgramNotFound(prog_name.to_owned()))?;
        let xdp: &mut Xdp = program
            .try_into()
            .map_err(|_| XdpLoaderError::NotXdp(prog_name.to_owned()))?;

        let mut attempts: u8 = 0;
        let mut last_err: Option<String> = None;
        for &mode in order {
            attempts = attempts.saturating_add(1);
            match xdp.attach(ifname, mode.to_flags()) {
                Ok(_link_id) => {
                    let label = mode.to_label();
                    crate::stats_export::record_attach_mode(label);
                    tracing::info!(
                        interface = ifname,
                        mode = label.as_str(),
                        attempts,
                        "xdp attached"
                    );
                    return Ok(AttachOutcome { mode, attempts });
                }
                Err(e) if is_unsupported_mode(&e) => {
                    tracing::warn!(
                        interface = ifname,
                        mode = mode.to_label().as_str(),
                        error = %e,
                        "xdp attach unsupported in this mode; trying next"
                    );
                    last_err = Some(format!("{e}"));
                    continue;
                }
                Err(e) => return Err(XdpLoaderError::from(e)),
            }
        }
        Err(XdpLoaderError::AllAttachModesExhausted(
            last_err.unwrap_or_else(|| "no attach attempts made".to_owned()),
        ))
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
    /// EBPF-2-03: the kernel-side map is `BPF_MAP_TYPE_LRU_HASH` (not
    /// plain HASH) so the kernel evicts the oldest entry when
    /// `max_entries` is reached instead of returning `ENOMEM` on
    /// `bpf_map_update_elem`. Aya 0.13.1's typed `HashMap` accessor
    /// accepts both `Map::HashMap` and `Map::LruHashMap` variants
    /// (see `aya/src/maps/mod.rs:505` — `HashMap from
    /// HashMap|LruHashMap`), so the insert/get API is unchanged.
    /// Callers should still keep ERROR-level handling on the `MapError`
    /// path but may downgrade "insert failed under pressure" log
    /// noise to WARN — LRU eviction is the expected steady state.
    ///
    /// # Errors
    ///
    /// - `MapNotFound` when the ELF does not declare `CONNTRACK`.
    /// - `Map` when aya rejects the map (size mismatch between Rust-side
    ///   `FlowKey`/`BackendEntry` and the ELF's declared sizes).
    pub fn conntrack_map(
        &mut self,
    ) -> Result<AyaHashMap<&mut MapData, FlowKey, BackendEntry>, XdpLoaderError> {
        // EBPF-2-05: lowercase pin-name (`#[map(name = "conntrack")]`
        // in the eBPF source). Aya `map_mut` accepts either case but
        // we use the on-disk-pin spelling to keep one source of
        // truth.
        let map = self
            .ebpf
            .map_mut(CONNTRACK_PIN_NAME)
            .ok_or(XdpLoaderError::MapNotFound(CONNTRACK_PIN_NAME))?;
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
            .map_mut(CONNTRACK_V6_PIN_NAME)
            .ok_or(XdpLoaderError::MapNotFound(CONNTRACK_V6_PIN_NAME))?;
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
            .map_mut(ACL_DENY_TRIE_PIN_NAME)
            .ok_or(XdpLoaderError::MapNotFound(ACL_DENY_TRIE_PIN_NAME))?;
        LpmTrie::try_from(map).map_err(Into::into)
    }

    /// Insert a CIDR deny rule into the IPv4 ACL LPM trie. `prefix_len` is
    /// the number of leading bits to match; `ipv4` is the address
    /// (network byte order handled internally). The stored value (`1`)
    /// is an opaque tag — the BPF program only cares about presence.
    ///
    /// ROUND8-L4-06: `prefix_len` is gated to `1..=32`. A `/0` entry
    /// would match every packet (default-deny footgun documented in
    /// the userspace simulator); `/33+` is structurally invalid for
    /// IPv4. Only the prefix is gated — a host-route like
    /// `insert_acl_deny(32, 0.0.0.0)` is accepted (zero IP at `/32`
    /// is a single host, not a wildcard).
    // TODO(L4-06): mirror this guard with `1..=128` when an IPv6
    // ACL trie ships (currently absent).
    ///
    /// # Errors
    ///
    /// - [`XdpLoaderError::InvalidAclPrefixV4`] if `prefix_len == 0`
    ///   or `prefix_len > 32`.
    /// - Propagates any error from [`XdpLoader::acl_trie`] plus
    ///   aya-level `bpf_map_update_elem` failures (full map,
    ///   permission denied).
    pub fn insert_acl_deny(
        &mut self,
        prefix_len: u8,
        ipv4: Ipv4Addr,
    ) -> Result<(), XdpLoaderError> {
        if prefix_len == 0 || prefix_len > 32 {
            return Err(XdpLoaderError::InvalidAclPrefixV4(prefix_len));
        }
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
    ///
    /// SEC-2-12: the license guard now runs before aya touches the
    /// bytes, so garbage trips [`XdpLoaderError::LicenseInvalid`]
    /// (the `object` parser rejects the unrecognised header). Either
    /// variant is acceptable here — the contract is "no panic on
    /// malformed input".
    #[test]
    fn load_garbage_bytes_rejected() {
        let garbage = [0u8; 16];
        let result = XdpLoader::load_from_bytes(&garbage);
        assert!(
            matches!(
                result,
                Err(XdpLoaderError::Load(_) | XdpLoaderError::LicenseInvalid(_))
            ),
            "expected Load or LicenseInvalid error for garbage bytes, got {result:?}",
        );
    }

    /// An empty slice is also invalid and must error.
    #[test]
    fn load_empty_bytes_rejected() {
        let empty: [u8; 0] = [];
        let result = XdpLoader::load_from_bytes(&empty);
        assert!(matches!(
            result,
            Err(XdpLoaderError::Load(_) | XdpLoaderError::LicenseInvalid(_))
        ));
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

    /// SEC-2-12: a well-formed ELF that lacks a `license` section
    /// must be rejected with [`XdpLoaderError::LicenseInvalid`].
    /// Uses a tiny hand-crafted ELF (header only, no sections of
    /// note) — large enough that `object::File::parse` accepts the
    /// header but small enough that `section_by_name("license")`
    /// returns `None`.
    #[test]
    #[allow(clippy::panic)] // crate-level lint, intentional in test code
    fn license_check_rejects_elf_without_license_section() {
        // 64-bit little-endian ELF header, type=REL, machine=BPF.
        // Section header table empty (e_shnum=0). `object` parses
        // this as a valid ELF with zero sections.
        let mut elf = vec![0u8; 64];
        elf[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        elf[4] = 2; // EI_CLASS = ELFCLASS64
        elf[5] = 1; // EI_DATA  = ELFDATA2LSB
        elf[6] = 1; // EI_VERSION = EV_CURRENT
        // e_type = ET_REL (1)
        elf[16..18].copy_from_slice(&1u16.to_le_bytes());
        // e_machine = EM_BPF (247)
        elf[18..20].copy_from_slice(&247u16.to_le_bytes());
        // e_version = EV_CURRENT (1)
        elf[20..24].copy_from_slice(&1u32.to_le_bytes());
        // e_ehsize = 64
        elf[52..54].copy_from_slice(&64u16.to_le_bytes());

        let result = assert_license_is_gpl(&elf);
        match result {
            Err(XdpLoaderError::LicenseInvalid(msg)) => {
                assert!(
                    msg.contains("license"),
                    "error must mention the missing section, got: {msg}",
                );
            }
            other => panic!("expected LicenseInvalid, got {other:?}"),
        }
    }

    /// SEC-2-12: an ELF whose `license` section contains the wrong
    /// bytes (e.g. `"BSD\0"`) must also be rejected. This is the
    /// case a misconfigured toolchain or accidental overwrite would
    /// produce.
    ///
    /// Constructs a minimal valid 64-bit LSB ELF with exactly one
    /// section named `license` whose payload is `"BSD\0"`. The
    /// section-name string table lives in section index 2.
    #[test]
    #[allow(clippy::panic)] // crate-level lint, intentional in test code
    fn license_check_rejects_wrong_payload() {
        let elf = build_elf_with_license_section(b"BSD\0");
        let result = assert_license_is_gpl(&elf);
        match result {
            Err(XdpLoaderError::LicenseInvalid(msg)) => {
                assert!(
                    msg.contains("BSD") || msg.contains("expected"),
                    "error must surface the actual bytes, got: {msg}",
                );
            }
            other => panic!("expected LicenseInvalid for BSD license, got {other:?}"),
        }
    }

    /// SEC-2-12: the happy path — a `license` section containing
    /// exactly `"GPL\0"` must be accepted.
    #[test]
    fn license_check_accepts_gpl_payload() {
        let elf = build_elf_with_license_section(b"GPL\0");
        let result = assert_license_is_gpl(&elf);
        assert!(
            result.is_ok(),
            "well-formed ELF with GPL license must pass, got {result:?}",
        );
    }

    /// Test helper: emit a minimal 64-bit LSB ELF with three
    /// sections (NULL, `.shstrtab`, `license`). The `license`
    /// section's content is `payload`.
    ///
    /// Kept inside `tests` so we can lean on `unwrap`/`expect` —
    /// the crate-level `#![deny(clippy::unwrap_used)]` is relaxed
    /// for test code by the `cfg_attr(test, allow(...))` pragma in
    /// `lib.rs`.
    fn build_elf_with_license_section(payload: &[u8]) -> Vec<u8> {
        // Layout:
        //   [0  ..64]   ELF header
        //   [64 ..64+N]  section data:
        //     [shstrtab payload]  "\0.shstrtab\0license\0"
        //     [license payload]   payload
        //   [...]   section header table (3 entries × 64 bytes)
        const EHDR_SIZE: usize = 64;
        const SHDR_SIZE: usize = 64;

        let shstr = b"\0.shstrtab\0license\0";
        let shstr_off = EHDR_SIZE;
        let shstr_size = shstr.len();
        let license_off = shstr_off + shstr_size;
        let license_size = payload.len();
        let shtab_off = license_off + license_size;

        let total = shtab_off + 3 * SHDR_SIZE;
        let mut elf = vec![0u8; total];

        // --- ELF header ---
        elf[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        elf[4] = 2; // ELFCLASS64
        elf[5] = 1; // ELFDATA2LSB
        elf[6] = 1; // EV_CURRENT
        elf[16..18].copy_from_slice(&1u16.to_le_bytes()); // e_type = ET_REL
        elf[18..20].copy_from_slice(&247u16.to_le_bytes()); // e_machine = EM_BPF
        elf[20..24].copy_from_slice(&1u32.to_le_bytes()); // e_version
        // e_shoff (64-bit) at offset 40
        elf[40..48].copy_from_slice(&(shtab_off as u64).to_le_bytes());
        elf[52..54].copy_from_slice(&(EHDR_SIZE as u16).to_le_bytes()); // e_ehsize
        elf[58..60].copy_from_slice(&(SHDR_SIZE as u16).to_le_bytes()); // e_shentsize
        elf[60..62].copy_from_slice(&3u16.to_le_bytes()); // e_shnum
        elf[62..64].copy_from_slice(&1u16.to_le_bytes()); // e_shstrndx = 1

        // --- section payloads ---
        elf[shstr_off..shstr_off + shstr_size].copy_from_slice(shstr);
        elf[license_off..license_off + license_size].copy_from_slice(payload);

        // --- section header table ---
        // Section 0: SHN_UNDEF — all zeros (already zeroed).

        // Section 1: .shstrtab
        let s1 = shtab_off + SHDR_SIZE;
        elf[s1..s1 + 4].copy_from_slice(&1u32.to_le_bytes()); // sh_name = 1 (".shstrtab")
        elf[s1 + 4..s1 + 8].copy_from_slice(&3u32.to_le_bytes()); // sh_type = SHT_STRTAB
        elf[s1 + 24..s1 + 32].copy_from_slice(&(shstr_off as u64).to_le_bytes()); // sh_offset
        elf[s1 + 32..s1 + 40].copy_from_slice(&(shstr_size as u64).to_le_bytes()); // sh_size

        // Section 2: license
        let s2 = shtab_off + 2 * SHDR_SIZE;
        elf[s2..s2 + 4].copy_from_slice(&11u32.to_le_bytes()); // sh_name = 11 ("license")
        elf[s2 + 4..s2 + 8].copy_from_slice(&1u32.to_le_bytes()); // sh_type = SHT_PROGBITS
        elf[s2 + 24..s2 + 32].copy_from_slice(&(license_off as u64).to_le_bytes()); // sh_offset
        elf[s2 + 32..s2 + 40].copy_from_slice(&(license_size as u64).to_le_bytes()); // sh_size

        elf
    }

    #[test]
    fn xdp_mode_is_copy() {
        let mode = XdpMode::Skb;
        let copy = mode;
        // Both usable after "move" — compiles iff XdpMode: Copy.
        assert_eq!(mode, copy);
    }
}
