//! Aya userspace XDP loader: parses a BPF ELF (from the standalone `lb-xdp-ebpf` crate) and,
//! on a privileged Linux host, attaches the XDP program to an interface.
//!
//! Linux-only — aya talks directly to the kernel's `bpf(2)` syscall.

use std::io;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use aya::{
    Ebpf, EbpfError, EbpfLoader, Pod,
    maps::{
        HashMap as AyaHashMap, Map, MapData, MapError,
        lpm_trie::{Key as LpmKey, LpmTrie},
    },
    programs::{ProgramError, Xdp, XdpFlags, xdp::XdpLinkId},
};
use aya_obj::{Object, ParseError};
use std::collections::HashMap as StdHashMap;

// SEC-2-12: the ELF is parsed a SECOND time with `object` (kernel-free) so the license
// assertion runs before aya's `EbpfLoader::load` ever touches the BPF syscall. `object::Object`
// and `aya_obj::Object` collide by name, hence the aliases.
use object::{File as ObjectFile, Object as ObjectTrait, ObjectSection as ObjectSectionTrait};

// EBPF-2-05: stable map pin names. These MUST match the `#[map(name = "...")]` strings in
// `crates/lb-l4-xdp/ebpf/src/main.rs` — aya creates `<pin_dir>/<NAME>`, and bpftool/cilium-cli
// read the same strings.

/// Pin filename of the IPv4 conntrack map.
pub const CONNTRACK_PIN_NAME: &str = "conntrack";

/// Pin filename of the IPv6 conntrack map.
pub const CONNTRACK_V6_PIN_NAME: &str = "conntrack_v6";

/// Pin filename of the L7 ports table (config-managed; not flood-pressured).
pub const L7_PORTS_PIN_NAME: &str = "l7_ports";

/// Pin filename of the IPv4 deny LPM trie.
pub const ACL_DENY_TRIE_PIN_NAME: &str = "acl_deny_trie";

/// Pin filename of the per-CPU stats array (EBPF-2-08 exposes the counter slots via `stats_export.rs`).
pub const STATS_PIN_NAME: &str = "stats";

/// ROUND8-L4-03: pin filename of the runtime new-flow-cap config (per-CPU `u32`). Userspace
/// writes `xdp_new_flow_cap_per_sec_per_cpu` here so the BPF `is_under_flood()` hot path reads
/// an operator-tunable cap without a redeploy; a `0` value disables the rate limiter.
pub const NEW_FLOW_CAP_CFG_PIN_NAME: &str = "new_flow_cap_cfg";

/// ROUND8-L4-03: pin filename of the per-CPU sliding-window counter. Owned by the BPF
/// program — userspace never writes it; named here so observability tooling finds the pin.
pub const NEW_FLOW_RATE_PIN_NAME: &str = "new_flow_rate";

/// ROUND8-L4-04: pin filename of the atomic per-VIP backend table, written one whole
/// `BackendTable` value per VIP with a single `bpf_map_update_elem` (Unimog / l4drop D1).
pub const BACKENDS_V4_PIN_NAME: &str = "backends_v4";

/// Default bpffs root for production. The directory must already exist as `0750` owned by the
/// LB uid:gid before the loader runs (see `crates/lb/src/xdp.rs` + the systemd unit); tests
/// override it with `EG_BPFFS_ROOT`.
pub const DEFAULT_PIN_DIR: &str = "/sys/fs/bpf/expressgateway";

// Userspace mirrors of the BPF map key/value layouts in `ebpf/src/main.rs`. They must stay in
// lock-step: aya compares their byte size against the ELF's declared map sizes when an accessor
// is constructed.

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
    /// Padding keeping the key 16 bytes wide for verifier alignment.
    ///
    /// CODE-2-07: `pub` only for back-compat with existing struct-literal sites. New code should
    /// use [`FlowKey::new`], which owns the zero-init contract.
    pub pad: [u8; 3],
}

// SAFETY: `FlowKey` is `#[repr(C)]`, `Copy`, and has no padding reads —
// aya's `Pod` is a marker trait requiring `Copy + 'static` layout stability.
unsafe impl Pod for FlowKey {}

impl FlowKey {
    /// Construct a [`FlowKey`] with explicit zero-initialised padding.
    ///
    /// CODE-2-07: funnelling callers through a constructor is the only way the zero-init
    /// property survives refactoring; `pad` is unconditionally `[0u8; 3]` and cannot be
    /// overridden.
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
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BackendEntry {
    /// Index into the per-service Maglev table in userspace.
    pub backend_idx: u32,
    /// Backend IPv4 address (network byte order) used by the `XDP_TX` rewrite.
    pub backend_ip: u32,
    /// Backend L4 port (network byte order).
    pub backend_port: u16,
    /// Padding — prefer [`BackendEntry::new`], which zero-initialises it.
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
    /// ROUND8-L4-01 caveat: infallible for back-compat — it does NOT reject the zero-IP /
    /// zero-port sentinels. New callers should use [`BackendEntry::try_new`]. The eBPF data
    /// plane mirrors the guard at runtime (`XDP_PASS` + a `backend_unpopulated` increment).
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

    /// ROUND8-L4-01: fallible constructor rejecting the `backend_ip == 0` / `backend_port == 0`
    /// sentinels. These are the Katran-lesson-10 silent-drop vector: a conntrack entry with a
    /// zero backend yields `XDP_TX` to 0.0.0.0:0, which the kernel drops without telemetry.
    /// The eBPF program enforces the same guard at runtime; this is the upstream admission gate.
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
    /// Padding to 40 bytes.
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
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BackendEntryV6 {
    /// Index into the userspace Maglev table.
    pub backend_idx: u32,
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

impl BackendEntryV6 {
    /// Construct a [`BackendEntryV6`] with zero-initialised padding. Like [`BackendEntry::new`]
    /// it is infallible and does NOT reject the zero sentinels — prefer `try_new`.
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

    /// ROUND8-L4-01: fallible IPv6 constructor rejecting `backend_ip == [0; 16]` (the
    /// unspecified address) and `backend_port == 0`. See [`BackendEntry::try_new`].
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
pub const BACKEND_ENTRY_SIZE: usize = 24;
/// Expected wire size of [`BackendEntryV6`] (matches BPF-side struct).
pub const BACKEND_ENTRY_V6_SIZE: usize = 36;

const _: () = assert!(core::mem::size_of::<FlowKey>() == FLOWKEY_SIZE);
const _: () = assert!(core::mem::size_of::<FlowKeyV6>() == FLOWKEY_V6_SIZE);
const _: () = assert!(core::mem::size_of::<BackendEntry>() == BACKEND_ENTRY_SIZE);
const _: () = assert!(core::mem::size_of::<BackendEntryV6>() == BACKEND_ENTRY_V6_SIZE);

/// ROUND8-L4-04: verifier-tractable ceiling on backends per VIP. MUST equal
/// `MAX_BACKENDS_PER_VIP` in `crates/lb-l4-xdp/ebpf/src/main.rs`.
pub const MAX_BACKENDS_PER_VIP: usize = 64;

/// ROUND8-L4-04: userspace mirror of the eBPF `BackendTable`. The whole struct is ONE map
/// value, so `publish_backends_v4` writes it with a single `bpf_map_update_elem` and a
/// concurrent data-plane lookup never sees a half-populated merge (Unimog / l4drop D1).
/// `previous_*` is the Unimog lesson-3 daisy-chain.
///
/// Layout MUST match the eBPF struct byte-for-byte — aya compares the Rust value size against
/// the ELF's declared map value size.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BackendTable {
    /// Monotonic publication counter (wraps; only equality matters).
    pub generation: u32,
    /// Live entry count (`<= MAX_BACKENDS_PER_VIP`).
    pub count: u32,
    /// Current generation's backends.
    pub entries: [BackendEntry; MAX_BACKENDS_PER_VIP],
    /// Daisy-chain: previous generation's live count (0 outside the transitional window).
    pub previous_count: u32,
    /// Explicit pad so the struct size is identical on both sides.
    pub pad: u32,
    /// Daisy-chain: previous generation's backends.
    pub previous_entries: [BackendEntry; MAX_BACKENDS_PER_VIP],
}

// SAFETY: `#[repr(C)] + Copy + 'static`; matches the eBPF layout
// (asserted below). `BackendEntry: Pod` already.
unsafe impl Pod for BackendTable {}

impl BackendTable {
    /// An all-zero table — the sentinel for `this VIP has never been published`, so the
    /// daisy-chain shift starts from a clean slate on the first publish.
    #[must_use]
    pub const fn zeroed() -> Self {
        const ZERO_ENTRY: BackendEntry = BackendEntry::new(0, 0, 0, [0u8; 6], [0u8; 6]);
        Self {
            generation: 0,
            count: 0,
            entries: [ZERO_ENTRY; MAX_BACKENDS_PER_VIP],
            previous_count: 0,
            pad: 0,
            previous_entries: [ZERO_ENTRY; MAX_BACKENDS_PER_VIP],
        }
    }
}

impl Default for BackendTable {
    fn default() -> Self {
        Self::zeroed()
    }
}

/// Expected wire size of [`BackendTable`]:
/// `4 + 4 + 24*64 + 4 + 4 + 24*64 = 3088`.
pub const BACKEND_TABLE_SIZE: usize = 4
    + 4
    + BACKEND_ENTRY_SIZE * MAX_BACKENDS_PER_VIP
    + 4
    + 4
    + BACKEND_ENTRY_SIZE * MAX_BACKENDS_PER_VIP;
const _: () = assert!(core::mem::size_of::<BackendTable>() == BACKEND_TABLE_SIZE);

/// XDP attach mode, mirroring the kernel's `XDP_FLAGS_*` bits. `Skb` works on any interface
/// (the CI/dev default); `Drv` needs NIC driver support and `Hw` needs hardware offload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XdpMode {
    /// Generic / SKB mode.
    Skb,
    /// Native driver mode.
    Drv,
    /// Hardware offload mode.
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

    /// EBPF-2-04: telemetry label for [`crate::stats_export`]. Kept symmetric with `XdpFlags`
    /// so a future kernel mode added to aya produces a compile error here.
    #[must_use]
    pub const fn to_label(self) -> crate::stats_export::AttachModeLabel {
        match self {
            Self::Skb => crate::stats_export::AttachModeLabel::Skb,
            Self::Drv => crate::stats_export::AttachModeLabel::Drv,
            Self::Hw => crate::stats_export::AttachModeLabel::Hw,
        }
    }
}

/// EBPF-2-04: classify a `ProgramError` as `mode unsupported by this NIC` — the ONLY errnos
/// that trigger ladder fall-through. Any other error is a real bug (verifier reject, bad
/// ifname) and must not be swallowed. EOPNOTSUPP=95 / EINVAL=22 are kernel-stable, coded as
/// literals to avoid a `libc` dependency.
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

    /// Object-level ELF parse failed (used by the kernel-free `parse_object_only` path).
    #[error("ebpf object parse error: {0}")]
    ObjectParse(#[from] ParseError),

    /// EBPF-2-04: the attach-mode ladder ran out of modes to try.
    #[error("all xdp attach modes exhausted; last error: {0}")]
    AllAttachModesExhausted(String),

    /// EBPF-2-08: installing the STATS per-CPU handle into the [`crate::stats_export`] module failed (already installed, or the map type didn't match `u64`).
    #[error("stats export install failed: {0}")]
    StatsExport(String),

    /// SEC-2-12: the ELF `license` section is missing or is not `GPL` + NUL. Most BPF helpers
    /// used here are `gpl_only`, so this fail-fast turns a deep `EACCES` at
    /// `bpf(BPF_PROG_LOAD)` into a clear startup error.
    #[error("bpf elf license check failed: {0}")]
    LicenseInvalid(String),

    /// ROUND8-L4-06: out-of-range CIDR prefix passed to [`XdpLoader::insert_acl_deny`]. The
    /// accepted range is `1..=32`: a `/0` deny is the block-everything footgun, `/33`+ is
    /// structurally nonsensical.
    #[error("invalid IPv4 ACL prefix length: got {0}, must be in 1..=32")]
    InvalidAclPrefixV4(u8),

    /// ROUND8-L4-01: `BackendEntry`/`BackendEntryV6` construction with `backend_ip == 0` or
    /// `backend_port == 0`. Katran lesson 10: a zero-IP backend causes a silent `XDP_TX` to
    /// 0.0.0.0:0 that the kernel drops invisibly. This is the construction-time admission gate.
    #[error("backend entry unpopulated: {reason}")]
    BackendUnpopulated {
        /// Operator-facing description (which field was zero).
        reason: &'static str,
    },

    /// ROUND8-L4-11: the `pin_dir` is not backed by bpffs. Pinning into a regular tmpfs makes
    /// aya deep-fail with an opaque EINVAL; this surfaces the actionable remediation instead.
    #[error("pin path {path:?} is not bpffs (found magic 0x{found_magic:x}); {hint}")]
    PinPathNotBpffs {
        /// The bad path the loader was asked to use.
        path: std::path::PathBuf,
        /// `statfs.f_type` value the kernel returned for the path.
        found_magic: i64,
        /// Operator-actionable next step (mount command).
        hint: String,
    },

    /// ROUND8-L4-11: the `statfs(2)` call on the pin directory itself failed (path missing, permission denied, ...).
    #[error("statfs on pin path {path:?} failed: {source}")]
    PinPathStatFailed {
        /// The path that could not be stat'd.
        path: std::path::PathBuf,
        /// Underlying I/O / errno-bearing error.
        #[source]
        source: io::Error,
    },

    /// ROUND8-L4-12: `XdpLoader::attach_replacing` found a foreign XDP program already attached to the interface.
    #[error("foreign XDP program attached: prog_id={0}; refusing to attach")]
    ForeignProgramAttached(u32),

    /// ROUND8-L4-12: an XDP program was expected but the kernel reports none. For
    /// `detach_verifying` this is the idempotent already-detached case; for `attach_replacing`
    /// it is a hard error (we cannot replace nothing).
    #[error("no XDP program attached to {0}")]
    NoProgramAttached(String),

    /// ROUND8-L4-12: the netlink `RTM_GETLINK` query itself failed. The caller cannot prove
    /// the kernel attach state, so `attach_replacing`/`detach_verifying` must NOT proceed blind.
    #[error("XDP netlink query failed for {iface}: {detail}")]
    XdpQueryFailed {
        /// Interface name.
        iface: String,
        /// Stringified underlying `io::Error`.
        detail: String,
    },

    /// ROUND8-L4-12: `detach_verifying` returned successfully but the post-detach kernel query still shows a program attached.
    #[error("detach left a program attached on {iface}: prog_id={prog_id:?}")]
    DetachLeftProgramAttached {
        /// Interface name.
        iface: String,
        /// Surviving prog_id (if any).
        prog_id: Option<u32>,
    },

    /// ROUND8-L4-04: more than [`MAX_BACKENDS_PER_VIP`] entries passed to
    /// [`XdpLoader::publish_backends_v4`]. Returned BEFORE any map write, so a too-large
    /// publish is a no-op and the live table is untouched.
    #[error("too many backends for one VIP: got {0}, max {max}", max = MAX_BACKENDS_PER_VIP)]
    TooManyBackends(usize),

    /// ROUND8-L4-05: the post-attach silent-drop probe or the static NIC blocklist found the
    /// requested mode dead (aya #1193 / Cilium lesson 8). The blocklist path demotes `Drv` →
    /// `Skb` and only surfaces this if `Skb` also fails.
    #[error("xdp attach probe failed in {mode:?} mode: {reason}")]
    AttachProbeFailed {
        /// The mode whose attach was found dead by the probe/blocklist.
        mode: XdpMode,
        /// Operator-facing reason incl.
        reason: String,
    },
}

/// SEC-2-12: required value of the ELF `license` section. Kernel-side `bpf_attr.license` is a
/// NUL-terminated C string that must equal `GPL`, so the section payload is exactly four bytes.
const EXPECTED_LICENSE: &[u8] = b"GPL\0";

/// SEC-2-12: parse the ELF and assert its `license` section is `GPL` + NUL. A free function so
/// unit tests can synthesise an ELF without the section and prove the assertion trips.
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

/// EBPF-2-04: outcome of [`XdpLoader::attach_with_fallback`] — the mode the kernel accepted
/// and how many ladder steps were tried. Surfaced as `xdp_attach_attempts_total`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttachOutcome {
    /// The mode the kernel accepted.
    pub mode: XdpMode,
    /// How many ladder steps were tried (>=1).
    pub attempts: u8,
}

/// EBPF-2-04: operator-facing knob mirroring [`lb_config::XdpModeChoice`], kept here so this
/// crate stays non-circular with `lb-config`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum XdpModeChoice {
    /// Drv → Skb fallback ladder.
    #[default]
    Auto,
    /// Drv only.
    Native,
    /// Skb only (today's behaviour pre-EBPF-2-04).
    Skb,
    /// Hw only.
    Hw,
}

/// High-level handle to a loaded BPF object containing an XDP program. Nothing is loaded into
/// the kernel until [`XdpLoader::attach`] is called.
#[derive(Debug)]
pub struct XdpLoader {
    ebpf: Ebpf,
    /// ROUND8-L4-12: link ids from `xdp.attach()`, keyed by `prog_name`. Retained so
    /// `detach_verifying` can issue a REAL `Xdp::detach(link_id)` — aya 0.13.1's detach consumes
    /// the id by value and `XdpLinkId` is not `Clone`.
    attached_links: StdHashMap<String, XdpLinkId>,
}

impl XdpLoader {
    /// Parse an in-memory BPF ELF and have aya create its declared maps in the kernel.
    pub fn load_from_bytes(elf: &[u8]) -> Result<Self, XdpLoaderError> {
        Self::load_from_bytes_pinned(elf, None)
    }

    /// EBPF-2-05: load with an optional `map_pin_path` so the maps survive a process restart.
    ///
    /// Aya reuses an existing pin when the kernel-side `map_type`/`key_size`/`value_size` match
    /// the ELF; on size mismatch it returns [`MapError::InvalidPin`], and recovering means
    /// unlinking the stale pin files and retrying. The caller owns creating the directory
    /// (`0750`, LB uid:gid).
    ///
    /// ROUND8-L4-11: the mount type is verified via [`crate::bpffs::assert_bpffs`], so a
    /// non-bpffs path returns [`XdpLoaderError::PinPathNotBpffs`] with a remediation hint
    /// instead of a deep-aya `InvalidPin` trail.
    pub fn load_from_bytes_pinned(
        elf: &[u8],
        pin_path: Option<&std::path::Path>,
    ) -> Result<Self, XdpLoaderError> {
        // SEC-2-12: verify the ELF declares license `GPL` BEFORE handing it to aya, so a bad
        // license is a clear startup error rather than EACCES inside `bpf(BPF_PROG_LOAD)`.
        assert_license_is_gpl(elf)?;
        let mut loader = EbpfLoader::new();
        if let Some(p) = pin_path {
            // ROUND8-L4-11: runs BEFORE `loader.map_pin_path(p)` so the operator sees the typed
            // error instead of the deep-aya EINVAL.
            crate::bpffs::assert_bpffs(p)?;
            loader.map_pin_path(p);
        }
        let ebpf = loader.load(elf)?;
        Ok(Self {
            ebpf,
            attached_links: StdHashMap::new(),
        })
    }

    /// EBPF-2-08: hand the STATS per-CPU array to [`crate::stats_export`].
    ///
    /// The map is TAKEN (not borrowed) so a second call cannot double-install it — single
    /// ownership matches the once-per-process invariant `crates/lb/src/xdp.rs` relies on.
    pub fn install_stats_export(&mut self) -> Result<(), XdpLoaderError> {
        let map = self.take_map(STATS_PIN_NAME)?;
        crate::stats_export::install_stats_handle(map)
            .map_err(|e| XdpLoaderError::StatsExport(e.to_string()))
    }

    /// ROUND8-L4-03: write the per-CPU new-flow cap into `new_flow_cap_cfg` so the BPF
    /// `is_under_flood()` hot path reads an operator-tunable threshold (Katran `MAX_CONN_RATE`
    /// parity). A `cap` of `0` DISABLES the rate limiter. Idempotent; the value is broadcast to
    /// every CPU's slot.
    pub fn set_new_flow_cap(&mut self, cap: u32) -> Result<(), XdpLoaderError> {
        use aya::maps::{PerCpuArray, PerCpuValues};
        let map = self
            .ebpf
            .map_mut(NEW_FLOW_CAP_CFG_PIN_NAME)
            .ok_or(XdpLoaderError::MapNotFound(NEW_FLOW_CAP_CFG_PIN_NAME))?;
        let mut cfg: PerCpuArray<&mut MapData, u32> =
            PerCpuArray::try_from(map).map_err(XdpLoaderError::Map)?;
        let nr_cpus = aya::util::nr_cpus()
            .map_err(|(_, e)| XdpLoaderError::Io(e))?
            .max(1);
        let values = PerCpuValues::try_from(vec![cap; nr_cpus]).map_err(XdpLoaderError::Io)?;
        cfg.set(0, values, 0).map_err(XdpLoaderError::Map)?;
        Ok(())
    }

    /// ROUND8-L4-04: typed accessor for the atomic per-VIP backend table map.
    pub fn backends_v4_map(
        &mut self,
    ) -> Result<AyaHashMap<&mut MapData, u32, BackendTable>, XdpLoaderError> {
        let map = self
            .ebpf
            .map_mut(BACKENDS_V4_PIN_NAME)
            .ok_or(XdpLoaderError::MapNotFound(BACKENDS_V4_PIN_NAME))?;
        AyaHashMap::try_from(map).map_err(Into::into)
    }

    /// ROUND8-L4-04: atomically publish a new backend set for `vip` (Unimog / l4drop D1).
    ///
    /// The whole `BackendTable` is written with a SINGLE `bpf_map_update_elem`, so a concurrent
    /// data-plane lookup observes either the entire previous or the entire new table — never a
    /// half-populated merge.
    ///
    /// Daisy-chain (Unimog lesson 3): the current `entries`/`count` shift into `previous_*`
    /// before the new set is written, so flows pinned to a now-old backend are still steerable
    /// during the transitional window. `generation` increments (wrapping) on every publish.
    pub fn publish_backends_v4(
        &mut self,
        vip: Ipv4Addr,
        new_entries: &[BackendEntry],
    ) -> Result<(), XdpLoaderError> {
        if new_entries.len() > MAX_BACKENDS_PER_VIP {
            return Err(XdpLoaderError::TooManyBackends(new_entries.len()));
        }
        let key = u32::from(vip).to_be();
        let mut map = self.backends_v4_map()?;
        // Read-modify-publish: the read is a point-in-time snapshot, the single insert below is
        // the atomic swap. One writer (the control plane), so no publish-publish race.
        let mut table = match map.get(&key, 0) {
            Ok(t) => t,
            Err(MapError::KeyNotFound) => BackendTable::zeroed(),
            Err(e) => return Err(XdpLoaderError::Map(e)),
        };
        // Daisy-chain shift: current → previous (Unimog lesson 3).
        table.previous_entries = table.entries;
        table.previous_count = table.count;
        // Repopulate `entries` from the new set; zero the tail so a
        // shrink cannot leave a stale backend addressable.
        let zero = BackendEntry::new(0, 0, 0, [0u8; 6], [0u8; 6]);
        table.entries = [zero; MAX_BACKENDS_PER_VIP];
        for (slot, e) in table.entries.iter_mut().zip(new_entries.iter()) {
            *slot = *e;
        }
        table.count = u32::try_from(new_entries.len()).unwrap_or(u32::MAX);
        table.generation = table.generation.wrapping_add(1);
        // ATOMIC publication: one syscall, whole value.
        map.insert(key, table, 0).map_err(XdpLoaderError::Map)?;
        Ok(())
    }

    /// Kernel-free ELF inspection: parse the BPF object with aya-obj and return every program name it declares.
    pub fn program_names(elf: &[u8]) -> Result<Vec<String>, XdpLoaderError> {
        let obj = Object::parse(elf)?;
        Ok(obj.programs.keys().cloned().collect())
    }

    /// Load an XDP program from the object into the kernel. Must be called before
    /// [`XdpLoader::attach`] for the named program.
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

    /// Attach the kernel-loaded XDP program to an interface. Requires a prior
    /// [`XdpLoader::kernel_load`], plus `CAP_BPF` + `CAP_NET_ADMIN` (older kernels:
    /// `CAP_SYS_ADMIN`).
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
        // ROUND8-L4-12: retain the XdpLinkId so `detach_verifying` can issue a real
        // `Xdp::detach(link_id)` and then VERIFY the interface is bare, rather than relying on
        // drop semantics.
        let link_id = xdp.attach(ifname, mode.to_flags())?;
        self.attached_links.insert(prog_name.to_owned(), link_id);
        Ok(())
    }

    /// EBPF-2-04: probe ladder for XDP attach.
    ///
    /// Falls back from Drv to Skb ONLY on `EOPNOTSUPP`/`EINVAL` (the two errnos meaning `this
    /// NIC does not support this mode`); any other error short-circuits so the real failure
    /// surfaces. `Native` and `Hw` intentionally SKIP the ladder — an operator who asked for
    /// Native gets a loud startup failure rather than a silent 10-50x regression to SKB.
    ///
    /// On success it calls [`crate::stats_export::record_attach_mode`].
    pub fn attach_with_fallback(
        &mut self,
        prog_name: &str,
        ifname: &str,
        requested: XdpModeChoice,
    ) -> Result<AttachOutcome, XdpLoaderError> {
        // Ladder definitions live here, NOT in the caller, so the policy is single-sourced and
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
        // ROUND8-L4-12: the link id is moved into `self.attached_links` only after the
        // `xdp`/`self.ebpf` borrow is released (XdpLinkId is not Clone).
        let mut succeeded: Option<(String, XdpLinkId, XdpMode)> = None;
        for &mode in order {
            attempts = attempts.saturating_add(1);
            // ROUND8-L4-05: static NIC blocklist gate. On a known-bad (driver, firmware) combo
            // we SKIP `Drv` entirely rather than attempt it — the attach syscall would SUCCEED
            // while the packet path silently goes to /dev/null (aya #1193 / Cilium lesson 8), so
            // failing the attach is not enough.
            if mode == XdpMode::Drv {
                if let Ok(crate::nic_compat::DrvSupport::Refuse { reason }) =
                    crate::nic_compat::drv_supported(ifname)
                {
                    tracing::warn!(
                        interface = ifname,
                        mode = "drv",
                        reason = %reason,
                        "xdp Drv refused by NIC blocklist (silent-drop \
                         class, aya#1193); demoting"
                    );
                    crate::stats_export::record_attach_probe_failed();
                    last_err = Some(format!("Drv refused by NIC blocklist: {reason}"));
                    continue;
                }
            }
            match xdp.attach(ifname, mode.to_flags()) {
                Ok(link_id) => {
                    let label = mode.to_label();
                    crate::stats_export::record_attach_mode(label);
                    tracing::info!(
                        interface = ifname,
                        mode = label.as_str(),
                        attempts,
                        "xdp attached"
                    );
                    // Record the link id via a deferred local, AFTER the `xdp` borrow of
                    // `self.ebpf` ends (ROUND8-L4-12).
                    succeeded = Some((prog_name.to_owned(), link_id, mode));
                    break;
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
        if let Some((name, link_id, mode)) = succeeded {
            self.attached_links.insert(name, link_id);
            return Ok(AttachOutcome { mode, attempts });
        }
        Err(XdpLoaderError::AllAttachModesExhausted(
            last_err.unwrap_or_else(|| "no attach attempts made".to_owned()),
        ))
    }

    /// Take ownership of a BPF map by name so the caller can access it through aya's typed map wrappers.
    pub fn take_map(&mut self, name: &'static str) -> Result<Map, XdpLoaderError> {
        self.ebpf
            .take_map(name)
            .ok_or(XdpLoaderError::MapNotFound(name))
    }

    /// Typed accessor for the IPv4 conntrack map, wrapping a mutable borrow of the underlying
    /// `MapData`.
    ///
    /// EBPF-2-03: the kernel-side map is `BPF_MAP_TYPE_LRU_HASH`, so the kernel evicts the
    /// oldest entry at `max_entries` instead of returning `ENOMEM`. Aya's typed `HashMap`
    /// accepts both variants, so the API is unchanged — but LRU eviction is now the expected
    /// steady state, so `insert failed under pressure` belongs at WARN, not ERROR.
    pub fn conntrack_map(
        &mut self,
    ) -> Result<AyaHashMap<&mut MapData, FlowKey, BackendEntry>, XdpLoaderError> {
        // EBPF-2-05: the lowercase on-disk pin spelling, so there is one source of truth.
        let map = self
            .ebpf
            .map_mut(CONNTRACK_PIN_NAME)
            .ok_or(XdpLoaderError::MapNotFound(CONNTRACK_PIN_NAME))?;
        AyaHashMap::try_from(map).map_err(Into::into)
    }

    /// Typed accessor for the IPv6 conntrack map.
    pub fn conntrack_v6_map(
        &mut self,
    ) -> Result<AyaHashMap<&mut MapData, FlowKeyV6, BackendEntryV6>, XdpLoaderError> {
        let map = self
            .ebpf
            .map_mut(CONNTRACK_V6_PIN_NAME)
            .ok_or(XdpLoaderError::MapNotFound(CONNTRACK_V6_PIN_NAME))?;
        AyaHashMap::try_from(map).map_err(Into::into)
    }

    /// Typed accessor for the IPv4 deny LPM trie (Pillar 4b-2 upgrade from the Pillar 4a `HashMap<u32, u32>`).
    pub fn acl_trie(&mut self) -> Result<LpmTrie<&mut MapData, u32, u32>, XdpLoaderError> {
        let map = self
            .ebpf
            .map_mut(ACL_DENY_TRIE_PIN_NAME)
            .ok_or(XdpLoaderError::MapNotFound(ACL_DENY_TRIE_PIN_NAME))?;
        LpmTrie::try_from(map).map_err(Into::into)
    }

    /// Insert a CIDR deny rule into the IPv4 ACL LPM trie. `prefix_len` is the number of
    /// leading bits to match; the stored value (`1`) is an opaque presence tag.
    ///
    /// ROUND8-L4-06: `prefix_len` is gated to `1..=32` — a `/0` would match every packet
    /// (the default-deny footgun) and `/33`+ is structurally invalid. Only the prefix is gated:
    /// `insert_acl_deny(32, 0.0.0.0)` is a single host route, not a wildcard.
    ///
    /// TODO(L4-06): mirror this guard with `1..=128` when an IPv6 ACL trie ships (absent today).
    pub fn insert_acl_deny(
        &mut self,
        prefix_len: u8,
        ipv4: Ipv4Addr,
    ) -> Result<(), XdpLoaderError> {
        if prefix_len == 0 || prefix_len > 32 {
            return Err(XdpLoaderError::InvalidAclPrefixV4(prefix_len));
        }
        // aya stores IPv4 addresses as `u32.to_be()` so the BPF side compares them
        // byte-for-byte against the packet's already-network-order src_addr.
        let key = LpmKey::<u32>::new(u32::from(prefix_len), u32::from(ipv4).to_be());
        let mut trie = self.acl_trie()?;
        trie.insert(&key, 1u32, 0).map_err(Into::into)
    }

    /// Borrow the underlying `Ebpf` object — escape hatch for callers that need full aya access (e.g. iterating all maps/programs).
    #[must_use]
    pub const fn ebpf(&self) -> &Ebpf {
        &self.ebpf
    }

    /// Mutably borrow the underlying `Ebpf` object.
    pub const fn ebpf_mut(&mut self) -> &mut Ebpf {
        &mut self.ebpf
    }

    // ROUND8-L4-12: attach-replace / detach-verifying API surface. The drain contract with
    // OPS-04 (`crates/lb/src/main.rs`) is ordered: cancel accept loops, drain in-flight tasks,
    // THEN call `detach_verifying(prog, iface, our_prog_id)` as the final step.

    /// ROUND8-L4-12: result of a kernel-side XDP query. `prog_id == None` means nothing is
    /// attached; `Some(_)` carries the kernel `bpf_prog_info.id` bound to IFLA_XDP.
    pub fn query_xdp(iface: &str) -> Result<XdpQueryResult, XdpLoaderError> {
        // ROUND8-L4-12: a REAL kernel query via netlink RTM_GETLINK (aya 0.13.1 exposes no
        // public `bpf_xdp_query`). This is what closes the EBUSY-on-redeploy hazard — the old
        // `prog_id: None` stub made every ownership/teardown check VACUOUS. The byte parser is
        // unit-tested against a captured real netlink blob; the live read needs no CAP_NET_ADMIN.
        let prog_id = crate::netlink_xdp::query_xdp_prog_id(iface).map_err(|e| {
            XdpLoaderError::XdpQueryFailed {
                iface: iface.to_owned(),
                detail: e.to_string(),
            }
        })?;
        Ok(XdpQueryResult {
            prog_id,
            mode: None,
        })
    }

    /// ROUND8-L4-12: attach with an explicit replace-of-known-prog-id. Verifies
    /// `query_xdp(iface).prog_id == Some(old_prog_id)` BEFORE attaching, so a co-resident
    /// third-party XDP program (e.g. Cilium) cannot be accidentally clobbered.
    pub fn attach_replacing(
        &mut self,
        prog_name: &str,
        iface: &str,
        mode: XdpMode,
        old_prog_id: u32,
    ) -> Result<AttachOutcome, XdpLoaderError> {
        // ROUND8-L4-12: this ownership check is REAL — `query_xdp` issues an actual RTM_GETLINK,
        // unlike the old `prog_id: None` stub that let everything through.
        let cur = Self::query_xdp(iface)?;
        match cur.prog_id {
            Some(id) if id == old_prog_id => {
                // Detach our previous link first (a real `Xdp::detach`), then re-attach: a fresh
                // attach over our own still-attached program returns EBUSY. A single-syscall
                // BPF_F_REPLACE would be ideal, but aya 0.13.1 exposes no wrapper.
                if let Some(link_id) = self.attached_links.remove(prog_name) {
                    let program = self
                        .ebpf
                        .program_mut(prog_name)
                        .ok_or_else(|| XdpLoaderError::ProgramNotFound(prog_name.to_owned()))?;
                    let xdp: &mut Xdp = program
                        .try_into()
                        .map_err(|_| XdpLoaderError::NotXdp(prog_name.to_owned()))?;
                    xdp.detach(link_id)?;
                }
                self.attach(prog_name, iface, mode)?;
                Ok(AttachOutcome { mode, attempts: 1 })
            }
            Some(id) => Err(XdpLoaderError::ForeignProgramAttached(id)),
            None => Err(XdpLoaderError::NoProgramAttached(iface.to_owned())),
        }
    }

    /// ROUND8-L4-12: detach with kernel-side verification — the signature OPS-04's drain
    /// coordinator calls as its final step. `Ok(())` only when the pre-detach query reports
    /// `Some(expected_prog_id)`, the aya detach succeeds, AND the post-detach query reports
    /// `None`.
    ///
    /// `ForeignProgramAttached` means someone else owns the interface (alert, leave it alone);
    /// `NoProgramAttached` is the idempotent already-detached case;
    /// `DetachLeftProgramAttached` is a kernel bug (alert ERR, force `ip link set dev <iface>
    /// xdp off`).
    pub fn detach_verifying(
        &mut self,
        prog_name: &str,
        iface: &str,
        expected_prog_id: u32,
    ) -> Result<(), XdpLoaderError> {
        // Step 1: REAL pre-detach query — confirm we own the interface before touching it.
        let pre = Self::query_xdp(iface)?;
        match pre.prog_id {
            Some(id) if id == expected_prog_id => {
                // Step 2: REAL detach. The old body was an empty block with NO `xdp.detach()`
                // call. With no tracked link (pin-loaded out of band) fall through to dropping
                // aya's managed link by removing the program.
                if let Some(link_id) = self.attached_links.remove(prog_name) {
                    let program = self
                        .ebpf
                        .program_mut(prog_name)
                        .ok_or_else(|| XdpLoaderError::ProgramNotFound(prog_name.to_owned()))?;
                    let xdp: &mut Xdp = program
                        .try_into()
                        .map_err(|_| XdpLoaderError::NotXdp(prog_name.to_owned()))?;
                    xdp.detach(link_id)?;
                }
            }
            Some(id) => return Err(XdpLoaderError::ForeignProgramAttached(id)),
            None => return Err(XdpLoaderError::NoProgramAttached(iface.to_owned())),
        }

        // Step 3: REAL post-detach query. Now that this is a genuine RTM_GETLINK, a surviving
        // prog_id is a true kernel-bug / racing-attacher signal, not a stub artefact.
        let post = Self::query_xdp(iface)?;
        if let Some(prog_id) = post.prog_id {
            return Err(XdpLoaderError::DetachLeftProgramAttached {
                iface: iface.to_owned(),
                prog_id: Some(prog_id),
            });
        }
        Ok(())
    }
}

/// ROUND8-L4-12: outcome of a kernel-side XDP attachment query. `prog_id == Some(id)` is the
/// kernel `bpf_prog_info.id` bound to the interface's IFLA_XDP attribute.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct XdpQueryResult {
    /// Kernel prog_id of the attached program, or `None` if none.
    pub prog_id: Option<u32>,
    /// Mode the kernel reports (drv / skb / hw); `None` if unknown.
    pub mode: Option<XdpMode>,
}

/// ROUND8-L4-03: Katran `MAX_CONN_RATE` parity default. Mirrors
/// `ebpf/src/main.rs::DEFAULT_NEW_FLOW_CAP_PER_CPU` and `lb_config`'s
/// `default_xdp_new_flow_cap_per_sec_per_cpu` — all three must move together.
pub const DEFAULT_NEW_FLOW_CAP_PER_SEC_PER_CPU: u32 = 125_000;

/// ROUND8-L4-03: userspace leaky-bucket limiter for control-plane conntrack inserts.
///
/// The BPF-side `is_under_flood()` gate protects the LRU from the attacker's data-plane RPS;
/// this is the mirror for the OTHER door — under a SYN flood `lb-balancer` would otherwise push
/// millions of throwaway CT entries/sec and thrash the LRU just the same.
///
/// `SystemTime` (not `Instant`) keeps the gate `Send + Sync + 'static`; the refill math only
/// uses deltas, so a wall-clock step backwards merely yields a safe zero-refill tick.
#[derive(Debug)]
pub struct CtInsertGate {
    tokens: AtomicU32,
    refill_per_sec: u32,
    burst: u32,
    last_refill_ns: AtomicU64,
}

impl CtInsertGate {
    /// Build a gate with `refill_per_sec` admissions/sec and a one-second burst ceiling. A
    /// `refill_per_sec` of `0` DISABLES the gate (every `try_admit` returns `true`), which is
    /// how `xdp_new_flow_cap_per_sec_per_cpu = 0` opts out.
    #[must_use]
    pub fn new(refill_per_sec: u32) -> Self {
        Self {
            tokens: AtomicU32::new(refill_per_sec),
            refill_per_sec,
            burst: refill_per_sec,
            last_refill_ns: AtomicU64::new(Self::now_ns()),
        }
    }

    fn now_ns() -> u64 {
        // A wall-clock step backwards saturating-subs to 0 in the refill path — never a panic.
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }

    /// Attempt to admit one control-plane conntrack insert. On `false` the caller MUST skip the
    /// insert and bump `StatSlot::NewFlowRateCap`.
    pub fn try_admit(&self) -> bool {
        if self.refill_per_sec == 0 {
            return true; // disabled
        }
        let now = Self::now_ns();
        let last = self.last_refill_ns.load(Ordering::Relaxed);
        let elapsed = now.saturating_sub(last);
        if elapsed > 0 {
            let refill =
                (u128::from(elapsed) * u128::from(self.refill_per_sec) / 1_000_000_000u128) as u64;
            if refill > 0
                && self
                    .last_refill_ns
                    .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
            {
                let add = u32::try_from(refill).unwrap_or(u32::MAX);
                let mut cur = self.tokens.load(Ordering::Relaxed);
                loop {
                    let next = cur.saturating_add(add).min(self.burst);
                    match self.tokens.compare_exchange_weak(
                        cur,
                        next,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => break,
                        Err(observed) => cur = observed,
                    }
                }
            }
        }
        let mut cur = self.tokens.load(Ordering::Relaxed);
        loop {
            if cur == 0 {
                return false;
            }
            match self.tokens.compare_exchange_weak(
                cur,
                cur - 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(observed) => cur = observed,
            }
        }
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

    /// Each `XdpMode` variant must map to exactly the expected aya flag set. `XdpFlags` has no
    /// `PartialEq`, so compare `.bits()`.
    #[test]
    fn xdp_mode_flag_mapping() {
        assert_eq!(XdpMode::Skb.to_flags().bits(), XdpFlags::SKB_MODE.bits());
        assert_eq!(XdpMode::Drv.to_flags().bits(), XdpFlags::DRV_MODE.bits());
        assert_eq!(XdpMode::Hw.to_flags().bits(), XdpFlags::HW_MODE.bits());
        assert_ne!(
            XdpMode::Skb.to_flags().bits(),
            XdpMode::Drv.to_flags().bits()
        );
        assert_ne!(
            XdpMode::Drv.to_flags().bits(),
            XdpMode::Hw.to_flags().bits()
        );
    }

    /// SEC-2-12: a well-formed ELF that lacks a `license` section must be rejected with [`XdpLoaderError::LicenseInvalid`].
    #[test]
    #[allow(clippy::panic)] // crate-level lint, intentional in test code
    fn license_check_rejects_elf_without_license_section() {
        // 64-bit LE ELF header, type=REL, machine=BPF, empty section header table (e_shnum=0):
        // `object` parses this as a valid ELF with zero sections.
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

    /// SEC-2-12: an ELF whose `license` section contains the wrong bytes (e.g. `"BSD\0"`) must also be rejected.
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

    /// SEC-2-12: the happy path — a `license` section containing exactly `"GPL\0"` must be accepted.
    #[test]
    fn license_check_accepts_gpl_payload() {
        let elf = build_elf_with_license_section(b"GPL\0");
        let result = assert_license_is_gpl(&elf);
        assert!(
            result.is_ok(),
            "well-formed ELF with GPL license must pass, got {result:?}",
        );
    }

    /// Test helper: emit a minimal 64-bit LSB ELF with three sections (NULL, `.shstrtab`, `license`).
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

        elf[shstr_off..shstr_off + shstr_size].copy_from_slice(shstr);
        elf[license_off..license_off + license_size].copy_from_slice(payload);

        // --- section header table ---
        // Section 0: SHN_UNDEF — all zeros (already zeroed).

        let s1 = shtab_off + SHDR_SIZE;
        elf[s1..s1 + 4].copy_from_slice(&1u32.to_le_bytes()); // sh_name = 1 (".shstrtab")
        elf[s1 + 4..s1 + 8].copy_from_slice(&3u32.to_le_bytes()); // sh_type = SHT_STRTAB
        elf[s1 + 24..s1 + 32].copy_from_slice(&(shstr_off as u64).to_le_bytes()); // sh_offset
        elf[s1 + 32..s1 + 40].copy_from_slice(&(shstr_size as u64).to_le_bytes()); // sh_size

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
