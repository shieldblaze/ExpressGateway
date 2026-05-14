//! ROUND8-L4-11: bpffs runtime check.
//!
//! Reference: xdp-tutorial lessons 6 + 7 — `mount -t bpf bpf
//! /sys/fs/bpf/` is a prerequisite for pinning. Without it, aya
//! `EbpfLoader::map_pin_path` ends up pinning into a regular tmpfs
//! and the kernel rejects it deep in `bpf(BPF_OBJ_GET)` with an
//! opaque EINVAL. Running an explicit `statfs(2)` on the pin
//! directory before handing it to aya gives the operator an
//! actionable error ("pin path is not bpffs; run `mount -t bpf …`")
//! instead of a kernel errno trail.
//!
//! Plan owns the bpffs runtime check; OPS-07 owns the systemd unit
//! that establishes the mount/perm contract this module verifies.
//!
//! Linux-only: the entire bpffs concept is a Linux kernel facility.

#![cfg(target_os = "linux")]

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::loader::XdpLoaderError;

/// Kernel magic for `BPF_FS_MAGIC` — `include/uapi/linux/magic.h`.
/// Stable kernel-side ABI; not exported by libc as a constant
/// (libc only ships POSIX statfs() symbols), so we redeclare the
/// constant here next to the use site.
pub const BPF_FS_MAGIC: i64 = 0xCAFE_4A11;

/// Verify that `path` resolves to a directory backed by bpffs.
///
/// On success returns `Ok(())`. On failure returns a typed
/// `XdpLoaderError` carrying enough context for the operator to
/// fix the deployment (which path, what magic the kernel reported,
/// suggested remediation).
///
/// # Errors
///
/// - [`XdpLoaderError::PinPathStatFailed`] when the `statfs(2)`
///   syscall itself fails (path missing, EACCES, ...).
/// - [`XdpLoaderError::PinPathNotBpffs`] when `statfs` succeeds but
///   the filesystem magic is not `BPF_FS_MAGIC` (e.g. someone tried
///   to pin into a plain tmpfs).
pub fn assert_bpffs(path: &Path) -> Result<(), XdpLoaderError> {
    // libc::statfs takes a NUL-terminated C string. Convert from
    // OsStr -> CString without unwrap; an interior NUL is itself
    // a hard error from the caller.
    let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|e| {
        XdpLoaderError::PinPathStatFailed {
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, e),
        }
    })?;

    // SAFETY: `statfs(2)` is async-signal-safe; the buffer is owned
    // and large enough; the path pointer is valid for the duration
    // of the call.
    let mut buf: libc::statfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statfs(c_path.as_ptr(), &mut buf) };
    if rc != 0 {
        return Err(XdpLoaderError::PinPathStatFailed {
            path: path.to_path_buf(),
            source: std::io::Error::last_os_error(),
        });
    }

    // `f_type` is `c_long` on 64-bit Linux glibc (i.e. i64). Cast
    // through the native width so the comparison against
    // `BPF_FS_MAGIC` works regardless of how libc exposes the field
    // on the target architecture.
    #[allow(
        clippy::unnecessary_cast,
        clippy::cast_lossless,
        clippy::useless_conversion,
        reason = "buf.f_type type varies across libc versions / arches"
    )]
    let fs_type_wide: i64 = buf.f_type as i64;

    if fs_type_wide != BPF_FS_MAGIC {
        return Err(XdpLoaderError::PinPathNotBpffs {
            path: path.to_path_buf(),
            found_magic: fs_type_wide,
            hint: "mount bpffs: `mount -t bpf bpffs /sys/fs/bpf` (or declare \
                   RequiresMountsFor=/sys/fs/bpf in the systemd unit so the \
                   service does not start before the mount is ready)"
                .to_owned(),
        });
    }

    Ok(())
}

/// Convenience: the production default pin directory.
#[must_use]
pub fn default_pin_dir() -> PathBuf {
    PathBuf::from(crate::loader::DEFAULT_PIN_DIR)
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    /// `assert_bpffs` on a regular tempdir (tmpfs or ext4) must
    /// return `PinPathNotBpffs`, not `Ok`. The CI runner's tempdir
    /// is by definition not bpffs.
    #[test]
    fn rejects_non_bpffs_tempdir() {
        let tmp = std::env::temp_dir();
        // Some CI sandboxes lock the temp dir to a non-statfs-able
        // path; skip the assertion in that case (the test is only
        // load-bearing when statfs succeeds).
        let result = assert_bpffs(&tmp);
        match result {
            Err(XdpLoaderError::PinPathNotBpffs {
                hint, found_magic, ..
            }) => {
                assert_ne!(found_magic, BPF_FS_MAGIC);
                assert!(
                    hint.contains("mount -t bpf"),
                    "hint must surface the mount command, got: {hint}"
                );
            }
            Err(XdpLoaderError::PinPathStatFailed { .. }) => {
                // Acceptable if the sandbox blocks statfs; the
                // contract is "no false-positive Ok on non-bpffs".
            }
            Ok(()) => panic!("tempdir cannot be bpffs but assert_bpffs returned Ok"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// Missing path returns `PinPathStatFailed` whose source kind
    /// is `NotFound` — the operator-visible message points at the
    /// missing directory.
    #[test]
    fn rejects_missing_path() {
        let path = Path::new("/nonexistent/bpf/expressgateway-test");
        match assert_bpffs(path) {
            Err(XdpLoaderError::PinPathStatFailed { source, path: p }) => {
                assert_eq!(p, path);
                assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
            }
            other => panic!("expected PinPathStatFailed(NotFound), got {other:?}"),
        }
    }

    /// Interior-NUL in the path is also a fail-fast, not a silent
    /// truncation — defensive against accidental injection.
    #[test]
    fn rejects_interior_nul_path() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let bad = PathBuf::from(OsString::from_vec(b"/tmp/\0nul".to_vec()));
        assert!(matches!(
            assert_bpffs(&bad),
            Err(XdpLoaderError::PinPathStatFailed { .. })
        ));
    }

    /// `BPF_FS_MAGIC` matches the kernel constant byte-for-byte.
    /// Sanity that no rebase / refactor changed the constant.
    #[test]
    fn bpf_fs_magic_constant_is_kernel_value() {
        assert_eq!(BPF_FS_MAGIC, 0xCAFE_4A11);
    }
}
